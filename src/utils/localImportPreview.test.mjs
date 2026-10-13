import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createLocalImportPreview, filterImportPreview, LOCAL_IMPORT_PREVIEW_LIMIT, safeImportPreviewImage, LOCAL_IMPORT_PREVIEW_IMAGE_MAX_BYTES } from "./localImportPreview.ts";
import { monitorConfigSave } from "./configSaveMonitor.ts";

const row = (n, source = "Nova") => ({ provisional_group_key: n.toString(16).padStart(64, "0"), source_id: source, album: "Mesmo título", artist: "Mesmo artista", discs: [], observed_tracks: 1 });
const sample = (albums, id = "1") => ({ operation_id: id, provisional: true, playable: false, complete: false, sampled_tracks: albums.length, sample_limit: 128, albums });
const operation = (phase = "indexing", id = "1") => ({ id, phase, error: null });
const flush = async () => { for (let n = 0; n < 8; n++) await Promise.resolve(); };
const png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aXioAAAAASUVORK5CYII=";
const imageRow = n => ({ ...row(n), image: png });

test("only bounded PNG data URLs with a PNG signature are accepted", () => {
  assert.equal(safeImportPreviewImage(png), png);
  for (const value of [null, undefined, "", 42, "https://example.org/cover.png",
    png.replace("image/png", "image/jpeg"), "data:image/png;base64,PHN2Zz48L3N2Zz4=",
    png + "\n", png.slice(0, -1), "data:image/png;base64,!!!!",
    "data:image/png;base64,iVBORw0KGgo" + "A".repeat(4 * Math.ceil(LOCAL_IMPORT_PREVIEW_IMAGE_MAX_BYTES / 3))]) {
    assert.equal(safeImportPreviewImage(value), null);
  }
  const bytes = Buffer.alloc(LOCAL_IMPORT_PREVIEW_IMAGE_MAX_BYTES);
  Buffer.from(png.split(",")[1], "base64").copy(bytes);
  assert.ok(safeImportPreviewImage(`data:image/png;base64,${bytes.toString("base64")}`));
  assert.equal(safeImportPreviewImage(`data:image/png;base64,${Buffer.concat([bytes, Buffer.from([0])]).toString("base64")}`), null);
});

test("images survive absent or invalid later samples and remain scoped to their group", async () => {
  const f = fixture();
  for (const rows of [[imageRow(1), row(2)], [{ ...row(1), image: null }],
    [row(1)], [{ ...row(1), image: "data:image/png;base64,invalid" }]]) {
    f.preview.observe(operation()); f.requests.at(-1).resolve(sample(rows)); await flush();
    assert.equal(f.current()[0].image, png);
    assert.equal(f.current()[1].image, null);
  }
  f.preview.dispose();
});

test("completion, replacement, clearing and disposal invalidate images and delayed replies", async () => {
  for (const action of ["completed", "failed", "uncertain", "replace", "clear", "dispose"]) {
    const f = fixture();
    f.preview.observe(operation()); f.requests[0].resolve(sample([imageRow(1)])); await flush();
    f.preview.observe(operation());
    if (action === "clear") f.preview.clear();
    else if (action === "dispose") f.preview.dispose();
    else f.preview.observe(action === "replace" ? operation("indexing", "2") : operation(action));
    const publications = f.publications.length;
    f.requests[1].resolve(sample([imageRow(2)])); await flush();
    assert.equal(f.publications.length, publications);
    if (action !== "dispose") assert.deepEqual(f.current(), []);
    if (action === "replace") {
      f.preview.observe(operation("indexing", "2"));
      f.requests[2].resolve(sample([row(1)], "2")); await flush();
      assert.equal(f.current()[0].image, null);
    }
    f.preview.dispose();
  }
});

function fixture() {
  const requests = [], publications = [];
  const preview = createLocalImportPreview(id => new Promise((resolve, reject) => requests.push({ id, resolve, reject })), rows => publications.push(rows));
  return { preview, requests, publications, current: () => publications.at(-1) ?? [] };
}

test("successive monitor polls discover, deduplicate and retain homonymous groups without a second poller", async () => {
  const f = fixture();
  const confirmed = [{ id: "confirmed", title: "Existing" }];
  const original = structuredClone(confirmed);
  const callbacks = [];
  let snapshot = operation();
  const monitor = monitorConfigSave(async () => snapshot, op => f.preview.observe(op), assert.fail,
    callback => { callbacks.push(callback); return callbacks.length; }, () => {});
  await flush();
  f.requests[0].resolve(sample([row(1)])); await flush();
  callbacks.shift()(); await flush();
  f.requests[1].resolve(sample([row(1), row(2)])); await flush();
  assert.deepEqual(f.current().map(a => a.provisional_group_key), [row(1).provisional_group_key, row(2).provisional_group_key]);
  callbacks.shift()(); await flush();
  f.requests[2].resolve(sample([row(3)])); await flush();
  assert.equal(f.current().length, 3);
  assert.deepEqual(confirmed, original);
  snapshot = operation("completed"); callbacks.shift()(); await flush();
  assert.deepEqual(f.current(), []);
  assert.equal(f.requests.length, 3);
  monitor.dispose(); f.preview.dispose();
});

test("bounded retention evicts oldest groups so later discoveries remain visible", async () => {
  const f = fixture();
  f.preview.observe(operation());
  f.requests[0].resolve(sample(Array.from({ length: 128 }, (_, n) => imageRow(n)))); await flush();
  assert.equal(f.current().length, LOCAL_IMPORT_PREVIEW_LIMIT);
  assert.equal(f.current()[0].provisional_group_key, row(80).provisional_group_key);
  assert.ok(f.current().every(album => album.image === png));
  f.preview.observe(operation()); f.requests[1].resolve(sample([row(128)])); await flush();
  assert.equal(f.current().length, LOCAL_IMPORT_PREVIEW_LIMIT);
  assert.equal(f.current().at(-1).provisional_group_key, row(128).provisional_group_key);
  assert.deepEqual(Object.keys(f.current()[0]).sort(), ["album", "artist", "image", "provisional_group_key", "source_id"]);
  f.preview.dispose();
});

test("completion, failure, rollback, uncertain and replacement discard pending replies", async () => {
  for (const next of [operation("validating"), operation("completed"), operation("failed"), operation("uncertain"), operation("indexing", "2")]) {
    const f = fixture();
    f.preview.observe(operation());
    f.requests[0].resolve(sample([row(1)])); await flush();
    assert.equal(f.current().length, 1);
    f.preview.observe(operation());
    f.preview.observe(next);
    assert.deepEqual(f.current(), []);
    f.requests[1].resolve(sample([row(3)])); await flush();
    assert.deepEqual(f.current(), []);
    if (next.id === "2") {
      f.preview.observe(next);
      assert.equal(f.requests[2].id, "2");
      f.requests[2].resolve(sample([row(2)], "2")); await flush();
      assert.equal(f.current().length, 1);
    } else {
      f.preview.observe(next);
      assert.equal(f.requests.length, 2);
    }
    f.preview.dispose();
  }
});

test("one request in flight; Busy/Unavailable/read failure never become Save errors", async () => {
  const f = fixture();
  for (const error of ["busy", "unavailable", "read_failed"]) {
    f.preview.observe(operation()); f.preview.observe(operation());
    const request = f.requests.at(-1);
    const count = f.requests.length;
    f.preview.observe(operation()); assert.equal(f.requests.length, count);
    request.reject(error); await flush();
    assert.deepEqual(f.current(), []);
  }
  f.preview.observe(operation());
  f.requests.at(-1).resolve(sample([row(1)])); await flush();
  assert.equal(f.current().length, 1);
  f.preview.dispose();
});

test("disposal, new acceptance, wrong operation and unsafe DTO do not publish", async () => {
  for (const action of ["dispose", "clear", "wrong", "playable", "oversized"]) {
    const f = fixture(); f.preview.observe(operation());
    let response = sample([row(1)]);
    if (action === "dispose") f.preview.dispose();
    if (action === "clear") f.preview.clear();
    if (action === "wrong") response.operation_id = "2";
    if (action === "playable") response.playable = true;
    if (action === "oversized") response.albums = Array.from({ length: 129 }, (_, n) => row(n));
    const count = f.publications.length;
    f.requests[0].resolve(response); await flush();
    assert.equal(f.publications.length, count);
    assert.deepEqual(f.current(), []);
    f.preview.dispose();
  }
});

test("scope and search filtering never merge provisional data into the confirmed catalog", () => {
  const rows = [row(1), row(2, "Outra")];
  assert.equal(filterImportPreview(rows, null).length, 2);
  assert.deepEqual(filterImportPreview(rows, "Outra"), [rows[1]]);
  assert.deepEqual(filterImportPreview(rows, "Anterior"), []);
  assert.equal(filterImportPreview(rows, null, "TÍTULO").length, 2);
});

const componentSource = readFileSync(new URL("../components/LocalImportPreviewSection.tsx", import.meta.url), "utf8");
// Layout composition renders with the existing grid substituted; its own
// measurement/range tests cover the browser-only virtualization separately.
const mockGridUrl = `data:text/javascript;base64,${Buffer.from(`
  import React from ${JSON.stringify(import.meta.resolve("react"))};
  export const calls = [];
  export function VirtualAlbumGrid(props) {
    calls.push(props);
    return React.createElement("div", {"data-grid": true}, props.items.map(item =>
      React.createElement("div", {key: props.getItemKey(item), "data-item": props.getItemKey(item)}, props.renderItem(item))));
  }
`).toString("base64")}`;
const gridMock = await import(mockGridUrl);
const componentJs = ts.transpileModule(componentSource, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText
  .replace(/from "(react|react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "../utils/localImportPreview"', `from ${JSON.stringify(new URL("./localImportPreview.ts", import.meta.url).href)}`)
  .replace('from "./VirtualAlbumGrid"', `from ${JSON.stringify(mockGridUrl)}`);
const { LocalImportPreviewSection } = await import(`data:text/javascript;base64,${Buffer.from(componentJs).toString("base64")}`);
const t = key => key;

test("embedded thumbnails stay decorative below the indexing mask; invalid images keep placeholders", () => {
  const markup = renderToStaticMarkup(React.createElement(LocalImportPreviewSection, {
    albums: [imageRow(1), row(2), { ...row(3), image: "https://example.org/cover.png" }], scrollContainer: {}, t,
  }));
  assert.equal((markup.match(/<img\b/g) ?? []).length, 1);
  assert.ok(markup.includes(`src="${png}"`));
  assert.ok(markup.includes('alt=""'));
  assert.ok(markup.includes('draggable="false"'));
  assert.ok(markup.indexOf("<img") < markup.indexOf("bg-black/40"));
  assert.equal((markup.match(/text-\[#555555\]/g) ?? []).length, 3);
  assert.doesNotMatch(markup, /<(button|a|input|audio)\b|tabindex=|href=|role="button"/i);
  assert.ok(componentSource.includes("onError={() => setFailed(true)}"));
  assert.ok(componentSource.includes("<PreviewImage key={image}"));
});

test("preview shares the existing grid/scroll and keeps placeholders without interactions/artwork requests", () => {
  const container = {};
  const markup = renderToStaticMarkup(React.createElement(LocalImportPreviewSection, { albums: [row(1), row(2)], scrollContainer: container, t }));
  assert.equal((markup.match(/data-item=/g) ?? []).length, 2);
  assert.equal(gridMock.calls.at(-1).scrollContainer, container);
  assert.ok(markup.includes("importPreviewIndexing"));
  assert.ok(markup.includes("bg-black/40"));
  assert.ok(markup.includes("animate-spin"));
  assert.doesNotMatch(markup, /<(button|a|img|input|audio)\b|tabindex=|href=|role="button"/i);
  assert.doesNotMatch(componentSource, /onClick|onKey|tabIndex|services\/|Artwork|LocalAlbumCard|\.uri|max-h-|overflow-y|grid-cols-|gap-4/);
  assert.equal(renderToStaticMarkup(React.createElement(LocalImportPreviewSection, { albums: [], scrollContainer: container, t })), "");
});

const browser = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
const branchStart = ') : viewMode === "albums" ? (';
const branchEnd = ') : viewMode === "artists" ? (';
const start = browser.indexOf(branchStart, browser.indexOf('ref={setScrollContainer}'));
const expression = browser.slice(start + branchStart.length, browser.indexOf(branchEnd, start));
// Exercise the actual album-branch JSX, including the empty-state conditions.
const layoutJs = ts.transpileModule(`function Layout(props) {
  const {filteredAlbums, visibleImportPreview, emptyCatalogState, scrollContainer} = props;
  const sortedAlbums = filteredAlbums, catalogActivity = "indexing", activeSourceId = null, albumSearch = "", albumSortMode = "album-asc";
  const albumGridRef = null, favoriteIds = new Set(), catalogConfirmed = false, isPlaybackAvailable = true, onlineArtworkEnabled = false, artworkBlocked = true, metadataMode = false;
  const handleToggleFavoriteLocal = () => {}, handleSelectAlbum = () => {}, handlePlayEntireAlbum = () => {};
  const localAlbumNavigationResetKey = () => "existing-navigation";
  const t = key => key;
  const Disc3 = () => null;
  const LocalAlbumCard = ({album}) => React.createElement("div", {"data-confirmed": album.id}, album.title);
  return (${expression});
}`, { compilerOptions: { jsx: ts.JsxEmit.React, target: ts.ScriptTarget.ES2020 } }).outputText;
const Layout = new Function("React", "VirtualAlbumGrid", "LocalImportPreviewSection", `${layoutJs}; return Layout;`)(React, gridMock.VirtualAlbumGrid, LocalImportPreviewSection);
const renderLayout = (confirmed, preview, state = "indexing", scrollContainer = {}) => renderToStaticMarkup(React.createElement(Layout, {
  filteredAlbums: confirmed, visibleImportPreview: preview, emptyCatalogState: state, scrollContainer,
}));

test("empty catalog displays provisional cards in the main scroll without an empty loading block", () => {
  const markup = renderLayout([], [row(1)]);
  assert.ok(markup.includes("importPreviewIndexing"));
  assert.doesNotMatch(markup, /h-60|indexingTooltip|emptyAlbums/);
  assert.ok(browser.indexOf('ref={setScrollContainer}') < browser.indexOf("<LocalImportPreviewSection"));
  assert.ok(renderLayout([], []).includes("indexingTooltip"));
  assert.doesNotMatch(renderLayout([], [row(1)], "error"), /h-60/);
  assert.ok(browser.includes('catalog.error && ('));
});

test("confirmed catalog retains its grid, navigation, order and shared scroll before the separate preview", () => {
  const confirmed = [{id: "b", title: "B", folder_path: "b"}, {id: "a", title: "A", folder_path: "a"}];
  const original = structuredClone(confirmed), container = {};
  gridMock.calls.length = 0;
  const markup = renderLayout(confirmed, [row(1)], "ready", container);
  assert.ok(markup.indexOf('data-confirmed="b"') < markup.indexOf('data-confirmed="a"'));
  assert.ok(markup.indexOf('data-confirmed="a"') < markup.indexOf("importPreviewTitle"));
  assert.deepEqual(gridMock.calls.map(call => call.scrollContainer), [container, container]);
  assert.equal(gridMock.calls[0].resetKey, JSON.stringify(["existing-navigation"]));
  assert.deepEqual(confirmed, original);
  assert.doesNotMatch(markup, /overflow-y|max-h-/);
});

test("multiple preview rows and filter changes use the same responsive grid without retaining stale cards", () => {
  const rows = Array.from({length: 48}, (_, n) => row(n, n < 24 ? "Nova" : "Outra"));
  for (const [source, search, count] of [[null, "", 48], ["Outra", "", 24], ["Outra", "absent", 0], ["Nova", "título", 24]]) {
    const markup = renderLayout([], filterImportPreview(rows, source, search));
    assert.equal((markup.match(/data-item=/g) ?? []).length, count);
  }
  assert.ok(browser.includes("filterImportPreview(importPreview, activeSourceId, albumSearch)"));
  const grid = readFileSync(new URL("../components/VirtualAlbumGrid.tsx", import.meta.url), "utf8");
  assert.ok(grid.includes("const MINIMUM_CARD_WIDTH = 170"));
  assert.ok(grid.includes("const GRID_GAP = 24"));
  assert.ok(grid.includes("const CARD_TEXT_HEIGHT = 64"));
});
