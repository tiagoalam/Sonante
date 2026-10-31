import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import ts from "typescript";
import { LocalAlbumThumbnails, thumbnailFromLoadedImage, LOCAL_ALBUM_THUMBNAILS_KEY,
  MAX_THUMBNAILS, MAX_THUMBNAIL_BYTES, MAX_THUMBNAILS_BYTES, THUMBNAIL_EDGE } from "./localAlbumThumbnails.ts";
import { localAlbumPresentationContext } from "./localAlbumPresentationCache.ts";
import { sortLocalAlbums } from "./localAlbumNavigation.ts";
import { artworkReadBlocked } from "./localArtwork.ts";

const album = id => ({ id, title: id, artist: "Artist", folder_path: `source/${id}`, source_id: "source", track_count: 1,
  discs: [{ number: 1, folder_path: `source/${id}`, label: "Disc", track_count: 1 }] });
// Finite JPEG SOF/EOI parser vectors, not a claim of browser image decoding.
const jpeg = (width = 160, height = 160, padding = 0) => {
  const header = [255,216,255,192,0,11,8,height >> 8,height & 255,width >> 8,width & 255,1,1,17,0];
  return "data:image/jpeg;base64," + Buffer.from([...header, ...new Uint8Array(padding), 255,217]).toString("base64");
};
const image = { complete: true, naturalWidth: 2000, naturalHeight: 1000, src: "data:image/png;base64,AAAA" };
function fixture(reduce = () => jpeg(), emit) {
  let value = null, writes = 0, removes = 0;
  const tasks = [];
  const storage = {
    getItem: key => { assert.equal(key, LOCAL_ALBUM_THUMBNAILS_KEY); return value; },
    setItem: (key, next) => { assert.equal(key, LOCAL_ALBUM_THUMBNAILS_KEY); value = next; writes++; },
    removeItem: key => { assert.equal(key, LOCAL_ALBUM_THUMBNAILS_KEY); value = null; removes++; },
  };
  const schedule = task => { const entry = { task, cancelled: false }; tasks.push(entry); return () => { entry.cancelled = true; }; };
  const cache = new LocalAlbumThumbnails(() => storage, reduce, schedule, emit);
  return { cache, storage, tasks, schedule, get value() { return value; }, get writes() { return writes; }, get removes() { return removes; } };
}

test("reduce only an already loaded data image, preserve aspect ratio and encode a small JPEG", () => {
  let drawn = 0;
  const canvas = { width: 0, height: 0,
    getContext: () => ({ fillRect() {}, drawImage(...args) { drawn++; assert.equal(args[0], image); assert.deepEqual(args.slice(1), [0,0,160,80]); } }),
    toDataURL: (mime, quality) => { assert.equal(mime, "image/jpeg"); assert.equal(quality, 0.7); return jpeg(160,80); } };
  assert.equal(thumbnailFromLoadedImage(image, () => canvas), jpeg(160,80));
  assert.equal(canvas.width, THUMBNAIL_EDGE); assert.equal(canvas.height, 80); assert.equal(drawn, 1);
  for (const src of ["https://server/cover?token=secret", "file:///cover", "asset:///cover", "data:image/svg+xml;base64,AAAA", "data:text/html;base64,AAAA"]) {
    assert.equal(thumbnailFromLoadedImage({ ...image, src }, () => { throw Error("must not draw"); }), null);
  }
  assert.equal(thumbnailFromLoadedImage({ ...image, complete: false }, () => canvas), null);
  assert.equal(thumbnailFromLoadedImage({ ...image, naturalWidth: 0 }, () => canvas), null);
  assert.equal(thumbnailFromLoadedImage(image, () => { throw Error("canvas"); }), null);
});

test("paint restored thumbnails without new reduction, lookups, writes or timers", () => {
  let reductions = 0;
  const f = fixture(() => { reductions++; return jpeg(); });
  f.cache.setContext(["/music"]); f.cache.capture(album("a"), image, () => true);
  assert.equal(f.writes, 0); assert.equal(f.tasks.length, 1); f.cache.flush();
  assert.equal(f.writes, 1); assert.equal(reductions, 1);
  const reopened = new LocalAlbumThumbnails(() => f.storage, () => { throw Error("paint must not reduce"); }, () => { throw Error("paint must not schedule"); });
  reopened.setContext(["/music"]);
  for (let i = 0; i < 10; i++) assert.equal(reopened.get(album("a")), jpeg());
  assert.equal(f.writes, 1); assert.equal(f.tasks.length, 1);
  reopened.capture(album("a"), image, () => false);
  assert.equal(f.writes, 1);
});

test("configuration and exact album binding prevent reuse by another source/album", () => {
  const f = fixture(); const a = album("a");
  f.cache.setContext(["/music"]); f.cache.capture(a, image, () => true); f.cache.flush();
  for (const changed of [{ ...a, id: "b" }, { ...a, source_id: "other" }, { ...a, title: "other" },
    { ...a, artist: "other" }, { ...a, folder_path: "other/a" }, { ...a, discs: [{ ...a.discs[0], folder_path: "other/disc" }] }]) {
    assert.equal(f.cache.get(changed), null);
  }
  f.cache.setContext(["/other"]); assert.equal(f.cache.get(a), null);
  f.cache.setContext(["/music"]); assert.equal(f.cache.get(a), jpeg());
  f.cache.setContext([]); assert.equal(f.cache.get(a), null);
});

test("corruption/version/URLs/large JPEG dimensions and oversized records become misses", () => {
  const f = fixture(); f.cache.setContext(["/music"]); f.cache.capture(album("a"), image, () => true); f.cache.flush();
  const good = JSON.parse(f.value);
  const invalid = ["{", "null", "{}", "x".repeat(MAX_THUMBNAILS_BYTES + 1),
    JSON.stringify({ ...good, version: 9 }), JSON.stringify({ ...good, images: [...good.images, ...good.images] })];
  for (const value of ["https://server/token", "data:image/svg+xml;base64,AAAA", "data:image/jpeg;base64,AAAA", jpeg(4096,4096), jpeg(0,1), jpeg(1,1,MAX_THUMBNAIL_BYTES)]) {
    invalid.push(JSON.stringify({ ...good, images: [[good.images[0][0], value]] }));
  }
  for (const value of invalid) {
    const reopened = new LocalAlbumThumbnails(() => ({ ...f.storage, getItem: () => value }), () => jpeg(), f.schedule);
    reopened.setContext(["/music"]); assert.equal(reopened.get(album("a")), null);
  }
});

test("opening order keeps the first available cards after exceeding the count or byte cap", () => {
  const albums = sortLocalAlbums(Array.from({ length: MAX_THUMBNAILS + 30 }, (_, i) => album(String(i))), "album-asc");
  assert.equal(MAX_THUMBNAILS, 250);
  for (const captureOrder of [albums, [...albums].reverse()]) {
    const f = fixture(); f.cache.setContext(["/music"]); f.cache.setOpeningOrder(albums);
    for (const item of captureOrder) f.cache.capture(item, image, () => true);
    assert.equal(f.tasks.length, 1); f.cache.flush();
    const kept = JSON.parse(f.value).images.map(([key]) => JSON.parse(key)[0]);
    assert.deepEqual(kept, albums.slice(0, MAX_THUMBNAILS).map(item => item.id));
    const reopened = new LocalAlbumThumbnails(() => f.storage, () => { throw Error("paint must not reduce"); }, f.schedule);
    reopened.setContext(["/music"]); reopened.setOpeningOrder(albums);
    for (const item of albums.slice(0, 24)) assert.equal(reopened.get(item), jpeg());
    assert.equal(reopened.get(albums[MAX_THUMBNAILS]), null);
  }
  const large = fixture(() => jpeg(160,160,16_000)); large.cache.setContext(["/music"]); large.cache.setOpeningOrder(albums);
  for (const item of [...albums].reverse()) large.cache.capture(item, image, () => true);
  large.cache.flush();
  const record = JSON.parse(large.value), kept = record.images.map(([key]) => JSON.parse(key)[0]);
  assert.ok(Buffer.byteLength(large.value) <= MAX_THUMBNAILS_BYTES);
  assert.ok(kept.length < MAX_THUMBNAILS);
  assert.deepEqual(kept, albums.slice(0, kept.length).map(item => item.id));
  for (const [, data] of record.images) assert.ok(Buffer.byteLength(data) <= MAX_THUMBNAIL_BYTES);
  assert.ok(large.cache.get(albums[0]));
});

test("changed config cancels old pending writes; late callbacks cannot flush the next context", () => {
  const f = fixture(); f.cache.setContext(["/music"]); f.cache.capture(album("old"), image, () => true);
  const old = f.tasks[0]; f.cache.setContext(["/other"]);
  assert.equal(old.cancelled, true); f.cache.capture(album("new"), image, () => true);
  old.task(); assert.equal(f.writes, 0);
  f.tasks[1].task(); assert.equal(f.writes, 1);
  assert.equal(JSON.parse(f.value).context, localAlbumPresentationContext(["/other"]));
  assert.equal(f.cache.get(album("old")), null);
  f.cache.capture(album("later"), image, () => true); f.cache.dispose(); f.tasks[2].task();
  assert.equal(f.writes, 1); assert.equal(f.cache.get(album("new")), null);
});

test("loss of current confirmation during reduction cannot publish the thumbnail", () => {
  let current = true;
  const f = fixture(() => { current = false; return jpeg(); }); f.cache.setContext(["/music"]);
  f.cache.capture(album("a"), image, () => current);
  assert.equal(f.cache.get(album("a")), null); assert.equal(f.tasks.length, 0);
  const changed = fixture(() => { changed.cache.setContext(["/other"]); return jpeg(); });
  changed.cache.setContext(["/music"]); changed.cache.capture(album("a"), image, () => true);
  assert.equal(changed.cache.get(album("a")), null);
});

test("quota/access failures do not affect normal display; only the thumbnail key is discarded", () => {
  const f = fixture(); f.cache.setContext(["/music"]); f.cache.capture(album("a"), image, () => true);
  f.storage.setItem = () => { throw Error("quota"); };
  assert.doesNotThrow(() => f.cache.flush()); assert.equal(f.removes, 1);
  assert.equal(f.cache.get(album("a")), jpeg());
  const unavailable = new LocalAlbumThumbnails(() => { throw Error("security"); }, () => jpeg(), f.schedule);
  unavailable.setContext(["/music"]); unavailable.capture(album("a"), image, () => true);
  assert.doesNotThrow(() => unavailable.flush());
});

const cardSource = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
const cardBody = ts.transpileModule(cardSource.slice(cardSource.indexOf("const LocalAlbumCard:"), cardSource.indexOf("export type LocalSubview")),
  { compilerOptions: { jsx: ts.JsxEmit.React, target: ts.ScriptTarget.ES2020 } }).outputText;
function renderCard({ confirmed, resolved, thumbnail }) {
  const a = album("a"), effects = []; let reductions = 0, lookups = 0, reads = 0, captures = 0, forgotten = 0; const triggers = [];
  const state = { albums: confirmed ? [a] : [], loaded: confirmed, loading: false, error: null, mpdBusy: false, saveBlocked: false };
  let stateIndex = 0;
  const component = new Function("useMemo", "useQueueInsertion", "loadLocalAlbumQueue", "React", "useTranslation", "useState", "useRef", "useEffect", "localArtworkResolver", "localAlbumThumbnails", "localAlbumCatalog", "artworkReadBlocked", "getLocalAlbumCover", "logError", "AlbumArtworkPlaceholder", "Heart", "Play", cardBody + "return LocalAlbumCard;")(
    fn => fn(), () => ({ open() {}, trigger: () => null, menu: null, notice: null }), async () => { throw new Error("unexpected album load"); }, React, () => ({ t: key => key }), initial => { const index = stateIndex++; return [index === 2 ? true : typeof initial === "function" ? initial() : initial, () => {}]; }, () => ({ current: null }), task => effects.push(task),
    { get: () => { lookups++; return resolved; }, subscribe: () => () => {} },
    { get: () => thumbnail, capture: (_album, _image, current, trigger) => { captures++; triggers.push(trigger); if (current()) reductions++; }, observeInitialCard() {}, forget: (_album, current) => { if (current()) forgotten++; } },
    { getSnapshot: () => state }, artworkReadBlocked, () => { reads++; return Promise.resolve(null); }, () => {}, () => null, () => null, () => null);
  const tree = component({ album: a, isFavorite: false, readEnabled: confirmed, isPlaybackAvailable: confirmed,
    isLibraryUpdating: false, onlineArtworkEnabled: false, onClick() {}, onPlayQuick() {}, onToggleFavorite() {} });
  const elements = [];
  function visit(node) { if (!React.isValidElement(node)) return; elements.push(node); React.Children.forEach(node.props.children, visit); }
  visit(tree);
  return { tree, elements, effects, state, triggers, get counts() { return { reductions, lookups, reads, captures, forgotten }; } };
}

test("the actual saved card paints a thumbnail with all actions and artwork requests inert", () => {
  const card = renderCard({ confirmed: false, resolved: "data:image/jpeg;base64,OLD_RAM", thumbnail: jpeg() });
  const img = card.elements.find(node => node.type === "img"); assert.equal(img.props.src, jpeg());
  assert.equal(card.tree.props.onClick, undefined);
  assert.ok(card.elements.filter(node => node.type === "button").every(node => node.props.disabled));
  card.effects[0](); card.effects[2](); img.props.onLoad({ currentTarget: { currentSrc: jpeg() } });
  assert.deepEqual(card.counts, { reductions: 0, lookups: 0, reads: 0, captures: 0, forgotten: 0 });
});

test("the actual confirmed card prefers the normal resolved cover and rechecks before capture", () => {
  const resolved = "data:image/png;base64,NORMAL";
  const card = renderCard({ confirmed: true, resolved, thumbnail: jpeg() });
  const img = card.elements.find(node => node.type === "img"); assert.equal(img.props.src, resolved);
  img.props.onLoad({ currentTarget: { currentSrc: resolved } });
  assert.equal(card.counts.reductions, 1);
  card.state.loaded = false; img.props.onLoad({ currentTarget: { currentSrc: resolved } });
  assert.equal(card.counts.reductions, 1);
  img.props.onLoad({ currentTarget: { currentSrc: "data:image/png;base64,LATE_OTHER" } });
  assert.equal(card.counts.captures, 2);
});


test("a confirmed missing cover retires its thumbnail; unconfirmed presentation cannot remove it", () => {
  const f = fixture(); f.cache.setContext(["/music"]); f.cache.capture(album("a"), image, () => true); f.cache.flush();
  f.cache.forget(album("a"), () => false); assert.equal(f.cache.get(album("a")), jpeg());
  f.cache.forget(album("a"), () => true); f.cache.flush();
  const reopened = new LocalAlbumThumbnails(() => f.storage, () => jpeg(), f.schedule);
  reopened.setContext(["/music"]); assert.equal(reopened.get(album("a")), null);
});

test("the thumbnail cannot short-circuit the actual card's normal lookup after confirmation", async () => {
  const card = renderCard({ confirmed: true, resolved: null, thumbnail: jpeg() });
  assert.equal(card.elements.find(node => node.type === "img").props.src, jpeg());
  card.effects[2](); await new Promise(resolve => setImmediate(resolve));
  assert.equal(card.counts.reads, 1); assert.equal(card.counts.forgotten, 1);
});


test("an already complete DOM image retries after onLoad was refused, without another lookup", () => {
  assert.equal((cardSource.match(/isLibraryUpdating=\{artworkBlocked\}/g) ?? []).length, 4);
  const resolved = "data:image/png;base64,NORMAL";
  const card = renderCard({ confirmed: true, resolved, thumbnail: null });
  const img = card.elements.find(node => node.type === "img");
  const loadedImage = { ...image, src: resolved, currentSrc: resolved };
  img.props.ref.current = loadedImage;
  card.state.loading = true;
  img.props.onLoad({ currentTarget: loadedImage });
  assert.equal(card.counts.reductions, 0);
  card.state.loading = false;
  card.effects[3]();
  assert.equal(card.counts.reductions, 1);
  assert.deepEqual(card.triggers, ["on_load", "already_complete"]);
  assert.equal(card.counts.reads, 0);
  card.state.albums = [];
  card.effects[3](); assert.equal(card.counts.reductions, 1, "a stale album still cannot capture");
});

test("a restored preview's complete image does not gain capture admission", () => {
  const card = renderCard({ confirmed: false, resolved: null, thumbnail: jpeg() });
  const img = card.elements.find(node => node.type === "img");
  img.props.ref.current = { ...image, src: jpeg(), currentSrc: jpeg() };
  card.effects[3](); card.effects[4]();
  assert.equal(card.counts.captures, 0); assert.equal(card.counts.reads, 0);
});

test("the same admitted DOM source is reduced only once; another context remains isolated", () => {
  let reductions = 0;
  const f = fixture(() => { reductions++; return jpeg(); });
  f.cache.setContext(["/music"]); f.cache.setOpeningOrder([album("a")]);
  f.cache.capture(album("a"), image, () => true, "on_load");
  f.cache.capture(album("a"), image, () => true, "already_complete");
  assert.equal(reductions, 1);
  f.cache.setContext(["/other"]);
  f.cache.capture(album("a"), image, () => true, "already_complete");
  assert.equal(reductions, 2);
});

test("DEV summaries distinguish restored/paint/capture/rejection/write failure without metadata", () => {
  const reports = [], reopenedReports = [], f = fixture(() => jpeg(), value => reports.push(value));
  const a = { ...album("secret-title"), title: "Private Title" };
  f.cache.setContext(["/private/music"]); f.cache.setOpeningOrder([a]);
  f.cache.observeInitialCard(a); f.cache.observeInitialCard(a);
  f.cache.capture(a, image, () => false, "already_complete");
  f.cache.capture(a, image, () => true, "already_complete");
  f.cache.flush();
  let diagnostic = f.cache.getDiagnostics();
  assert.equal(diagnostic.restore, "absent"); assert.equal(diagnostic.initial_cards, 1);
  assert.equal(diagnostic.initial_misses, 1); assert.equal(diagnostic.rejected.admission, 1);
  assert.equal(diagnostic.captured, 1); assert.equal(diagnostic.stored, 1);
  const reopened = new LocalAlbumThumbnails(() => f.storage, () => jpeg(), f.schedule, value => reopenedReports.push(value));
  reopened.setContext(["/private/music"]); reopened.setOpeningOrder([a]); reopened.observeInitialCard(a);
  diagnostic = reopened.getDiagnostics(); assert.equal(diagnostic.restore, "restored");
  assert.equal(diagnostic.restored, 1); assert.equal(diagnostic.initial_hits, 1);
  f.storage.setItem = () => { throw Error("private error message"); };
  f.cache.capture(album("other-private"), image, () => true); f.cache.flush();
  assert.equal(f.cache.getDiagnostics().write_failures, 1);
  for (let i = 0; i < 20; i++) {
    f.cache.capture(album(String(i)), image, () => true); f.cache.flush();
    for (const entry of f.tasks.splice(0)) if (!entry.cancelled) entry.task();
  }
  f.cache.finishDiagnostics(); f.cache.finishDiagnostics();
  assert.ok(reports.length <= 6); assert.ok(reopenedReports.length <= 6);
  assert.doesNotMatch(JSON.stringify([...reports, ...reopenedReports]), /private|Private Title|secret-title|data:image|source\//);
  for (const value of reports) for (const [key, count] of Object.entries(value)) {
    if (!["stage", "restore", "rejected"].includes(key)) assert.ok(Number.isSafeInteger(count) && count >= 0);
  }
  const session = readFileSync(new URL("./localArtworkSession.ts", import.meta.url), "utf8");
  assert.match(session, /import \{ logLocalThumbnailDiagnostic \} from "\.\/frontendDiagnostics"/);
  assert.match(session, /import.meta.env\?\.DEV \? logLocalThumbnailDiagnostic : undefined/);
});

test("reduction failures report only the fixed reason, never exceptions or image payload", () => {
  const reports = [], f = fixture((input, _canvas, reject) => thumbnailFromLoadedImage(input, () => { throw Error("secret canvas error"); }, reject), value => reports.push(value));
  f.cache.setContext(["/music"]);
  f.cache.capture(album("a"), image, () => true);
  f.cache.capture(album("a"), { ...image, src: "https://private/?token=secret" }, () => true);
  assert.equal(f.cache.getDiagnostics().rejected.canvas_failed, 1);
  assert.equal(f.cache.getDiagnostics().rejected.source_type, 1);
  f.cache.finishDiagnostics();
  assert.doesNotMatch(JSON.stringify(reports), /private|secret|https|data:image/);
});


test("the JPEG parser accepts a real encoded image with APP/DQT/Huffman segments", () => {
  const encoded = 'data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAABAAEDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDx+iiigD//2Q==';
  const f = fixture(() => encoded); f.cache.setContext(["/music"]);
  f.cache.capture(album("a"), image, () => true); f.cache.flush();
  assert.equal(f.cache.get(album("a")), encoded); assert.equal(f.cache.getDiagnostics().captured, 1);
});
