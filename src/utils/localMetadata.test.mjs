import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { validCatalogTagPresence, catalogPresenceCategory, catalogPresenceCounts, filterCatalogPresence } from "./localMetadataPresence.ts";

const source = path => readFileSync(new URL(path, import.meta.url), "utf8");
const dataModule = code => `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
const compile = text => ts.transpileModule(text, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
const hooksUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))}; export default React;
 ${["useState", "useRef", "useEffect", "useMemo", "useSyncExternalStore"].map(name => `export const ${name} = (...args) => globalThis.__metadataSpaceHost.${name}(...args);`).join("\n")}`);
const translationUrl = dataModule('export const useTranslation = () => ({ t: (...args) => globalThis.__metadataSpaceTranslation.t(...args) });');
const translations = {};
for (const language of ["pt-BR", "en"]) {
  const instance = i18next.createInstance();
  await instance.init({ lng: language, resources: { [language]: { translation: JSON.parse(source(`../locales/${language}.json`)) } }, interpolation: { escapeValue: false } });
  translations[language] = instance;
}
const audioUrl = dataModule('export const audioService = new Proxy({}, { get: (_, method) => (...args) => globalThis.__metadataSpaceAudio(method, args) });');
const favoritesUrl = dataModule('export const favoritesService = { getFavorites: () => Promise.resolve([]) };');
const artworkUrl = dataModule(`const status = {onlineActivity: null}; export const localArtworkEnrichment = {
 subscribe: () => () => {}, getSnapshot: () => status, setCatalog: () => globalThis.__metadataSpaceReads.push("artwork.setCatalog") };
 export const localArtworkResolver = {get: () => null, subscribe: () => () => {}, resolve: async () => { globalThis.__metadataSpaceReads.push("artwork.resolve"); return null; }};`);
const virtualUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))};
 export const VirtualList = props => React.createElement("div", {"data-virtual-list": true}, props.items.map(item => React.createElement(React.Fragment, {key: props.getItemKey(item)}, props.renderItem(item))));
 export const VirtualAlbumGrid = () => null;`);
const componentMocks = new Map([
  ["audio.ts", audioUrl], ["favorites.ts", favoritesUrl], ["localArtworkSession.ts", artworkUrl],
  ["VirtualList.tsx", virtualUrl], ["VirtualAlbumGrid.tsx", virtualUrl],
  ...["LocalAlbumTagEditorModal", "LibraryDoctorModal", "LocalTagEditorModal", "PlaylistPickerModal", "LocalImportPreviewSection", "AlbumArtworkPlaceholder", "ToolbarSelect", "AlbumGenreGrid"].map(name => [name + ".tsx", dataModule(`export const ${name} = () => null;`)]),
]);
const moduleCache = new Map();
function moduleUrl(url) {
  const path = new URL(url); const filename = path.pathname.split("/").at(-1);
  if (componentMocks.has(filename)) return componentMocks.get(filename);
  if (moduleCache.has(path.href)) return moduleCache.get(path.href);
  const code = compile(readFileSync(path, "utf8")).replace(/from "([^"]+)"/g, (_, name) => {
    if (name === "react") return `from ${JSON.stringify(hooksUrl)}`;
    if (name === "react-i18next") return `from ${JSON.stringify(translationUrl)}`;
    if (!name.startsWith(".")) return `from ${JSON.stringify(import.meta.resolve(name))}`;
    let next = new URL(name, path);
    if (!existsSync(next)) next = [".ts", ".tsx"].map(extension => new URL(name + extension, path)).find(existsSync);
    assert.ok(next, name); return `from ${JSON.stringify(moduleUrl(next))}`;
  });
  const result = dataModule(code); moduleCache.set(path.href, result); return result;
}
const { LocalBrowserView, localAlbumCatalog } = await import(moduleUrl(new URL("../components/LocalBrowserView.tsx", import.meta.url)));
const { LocalMetadataView } = await import(moduleUrl(new URL("../components/LocalMetadataView.tsx", import.meta.url)));
const { VirtualList, VirtualAlbumGrid } = await import(virtualUrl);
const { LocalAlbumTagEditorModal: Editor } = await import(componentMocks.get("LocalAlbumTagEditorModal.tsx"));
const { LibraryDoctorModal: Doctor } = await import(componentMocks.get("LibraryDoctorModal.tsx"));
const { LocalTagEditorModal: TrackEditor } = await import(componentMocks.get("LocalTagEditorModal.tsx"));
const { SidebarNavItem } = await import(moduleUrl(new URL("../components/SidebarNavItem.tsx", import.meta.url)));
const sampleAlbum = id => ({ id, title: "Same title", artist: "Same artist", year: "2004", source_id: id,
  folder_path: `Synthetic/${id}`, track_count: id === "A" ? 3 : 5, discs: [{ number: 1, label: "Disc", folder_path: `Synthetic/${id}`, track_count: id === "A" ? 3 : 5 }] });
const readyCatalog = () => ({ albums: [sampleAlbum("A"), sampleAlbum("B")], loaded: true, loading: false, error: null, mpdBusy: false, saveBlocked: false });
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function host(Component, props) {
  let cursor = 0; const slots = [], effects = [];
  const h = {
    useState(initial) { const i = cursor++; if (!(i in slots)) slots[i] = typeof initial === "function" ? initial() : initial; return [slots[i], value => slots[i] = typeof value === "function" ? value(slots[i]) : value]; },
    useRef(initial) { return slots[cursor++] ??= { current: initial }; },
    useMemo(factory, deps) { const i = cursor++; if (!slots[i] || deps.some((value, j) => value !== slots[i].deps[j])) slots[i] = { value: factory(), deps }; return slots[i].value; },
    useEffect(setup, deps) { const i = cursor++, previous = slots[i]; if (!previous || !deps || deps.some((value, j) => value !== previous.deps[j])) effects.push(() => { previous?.cleanup?.(); slots[i] = { deps, cleanup: setup() }; }); },
    useSyncExternalStore(_, getSnapshot) { return getSnapshot(); },
    render(next = props) { props = next; globalThis.__metadataSpaceHost = h; cursor = 0; return Component(props); },
    effects() { effects.splice(0).forEach(setup => setup()); }, cleanup() { slots.forEach(slot => slot?.cleanup?.()); }, slots,
  }; return h;
}
function nodes(tree, predicate) {
  if (Array.isArray(tree)) return tree.flatMap(node => nodes(node, predicate));
  if (!tree || typeof tree !== "object") return [];
  return [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
}
const get = (tree, type) => nodes(tree, node => node.type === type)[0];
const button = (tree, text) => nodes(tree, node => node.type === "button" && React.Children.toArray(node.props.children).includes(text))[0];
function fixture(extra = {}) {
  globalThis.__metadataSpaceTranslation = translations.en; const reads = globalThis.__metadataSpaceReads = [];
  localAlbumCatalog.state = readyCatalog();
  localAlbumCatalog.updating = false; localAlbumCatalog.inFlight = localAlbumCatalog.pending = localAlbumCatalog.waitingForMpd = false;
  globalThis.__metadataSpaceAudio = async method => { reads.push(method); return method === "getLocalAlbums" ? localAlbumCatalog.getSnapshot().albums : method === "getLocalAlbumTracks" || method === "listLocalDirectory" ? [] : null; };
  const callbacks = [], props = { subview: "metadata", catalogRefreshRevision: 0, isPlaybackAvailable: false, isLibraryUpdating: false,
    onlineArtworkEnabled: true, configuredLocalFolders: [], onSubviewChange: view => callbacks.push(view), onPlayerNavigationBack: () => callbacks.push("player-back"), ...extra };
  const h = host(LocalBrowserView, props);
  const metadata = tree => get(tree, LocalMetadataView).props;
  return { h, props, reads, callbacks, metadata, async settle(next = props) { for (let i = 0; i < 3; i++) { h.render(next); h.effects(); await flush(); } return h.render(next); } };
}

test("Metadata renders searchable virtual rows with exact IDs, catalog-only totals and explicit actions in both languages", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__metadataSpaceTranslation = translations[language]; const calls = [], catalog = readyCatalog();
    catalog.albums[1].artist = "Outro artista";
    const props = { catalog, updating: false, selectedAlbumId: "B", dialogsOpen: false,
      onSelectAlbum: id => calls.push(["select", id]), onDiagnose: id => calls.push(["diagnose", id]), onReview: id => calls.push(["review", id]), onBack() {} };
    const h = host(LocalMetadataView, props), t = translations[language].t.bind(translations[language]);
    let tree = h.render(); assert.equal(calls.length, 0); const list = get(tree, VirtualList);
    assert.deepEqual(list.props.items.map(list.props.getItemKey), ["A", "B"]);
    const row = list.props.renderItem(catalog.albums[1]); assert.equal(row.props["aria-pressed"], true); row.props.onClick();
    button(tree, t("localMetadata.diagnose")).props.onClick(); button(tree, t("localMetadata.review")).props.onClick();
    assert.deepEqual(calls, [["select", "B"], ["diagnose", "B"], ["review", "B"]]);
    const markup = renderToStaticMarkup(tree);
    assert.ok(markup.includes(t("localMetadata.catalogTotals", { albums: 2, tracks: 8 })));
    assert.ok(markup.includes(t("localMetadata.albumTotals", { tracks: 5, discs: 1 })));
    assert.doesNotMatch(markup, /<img|libraryDoctor\.codes|inspection_complete|physical_tracks_read/);
    nodes(tree, node => node.type === "input" && node.props.type === "search")[0].props.onChange({ target: { value: "outro" } }); tree = h.render();
    assert.deepEqual(get(tree, VirtualList).props.items.map(album => album.id), ["B"]);
    nodes(tree, node => node.type === "input" && node.props.type === "search")[0].props.onChange({ target: { value: "no match" } });
    assert.ok(renderToStaticMarkup(h.render()).includes(t("localMetadata.noMatches"))); h.cleanup();
  }
});

test("Metadata distinguishes loading/error/empty/update/transaction/unavailable selection and blocks actions", () => {
  globalThis.__metadataSpaceTranslation = translations.en; const t = translations.en.t.bind(translations.en);
  for (const [patch, updating, key] of [
    [{ loaded: false, loading: true }, false, "loading"], [{ error: "Synthetic failure" }, false, "error"],
    [{ albums: [] }, false, "empty"], [{ mpdBusy: true }, false, "updating"], [{}, true, "updating"],
    [{ saveBlocked: true }, false, "saveBlocked"], [{ albums: [] }, false, "albumUnavailable"],
  ]) {
    const props = { catalog: { ...readyCatalog(), ...patch }, updating, selectedAlbumId: "B", dialogsOpen: false, onSelectAlbum() {}, onDiagnose() {}, onReview() {}, onBack() {} };
    const h = host(LocalMetadataView, props), tree = h.render(), markup = renderToStaticMarkup(tree);
    assert.ok(markup.includes(t(`localMetadata.${key}`)), key);
    for (const action of ["diagnose", "review"]) { const b = button(tree, t(`localMetadata.${action}`)); if (b) assert.equal(b.props.disabled, true, key); }
    if (patch.loaded === false) assert.ok(markup.includes(t("localMetadata.presence.lastSnapshot")));
    h.cleanup();
  }
});

test("selecting Metadata albums reuses the catalog and causes no automatic artwork, track, inspection, online or refresh I/O", async () => {
  const f = fixture(); let tree = await f.settle(); assert.deepEqual(f.reads, []);
  const shared = localAlbumCatalog.getSnapshot(); f.metadata(tree).onSelectAlbum("B"); tree = await f.settle();
  assert.equal(f.metadata(tree).selectedAlbumId, "B"); assert.equal(f.metadata(tree).catalog, shared); assert.deepEqual(f.reads, []);
  assert.equal(get(tree, Editor), undefined); assert.equal(get(tree, Doctor), undefined);
  f.metadata(tree).onSelectAlbum("missing"); assert.equal(f.metadata(f.h.render()).selectedAlbumId, "B"); f.h.cleanup();
});

test("album/player review enters Metadata with exact album identity and retains browser context without extra reads", async () => {
  for (const fromPlayer of [false, true]) {
    const a = readyCatalog().albums[1], target = { kind: "local", view: "album", albumId: "B", verified: { catalog: [], groups: [] } };
    const f = fixture({ subview: "browser", ...(fromPlayer ? { playerTarget: target } : {}) });
    let tree = await f.settle();
    if (!fromPlayer) { get(tree, VirtualAlbumGrid).props.renderItem(localAlbumCatalog.getSnapshot().albums[1]).props.onClick(); tree = await f.settle(); }
    const browseHeading = nodes(tree, n => n.type === "h1")[0].props.children;
    f.reads.length = 0;
    const entry = nodes(tree, n => n.type === "button" && n.props.title === translations.en.t("localMetadata.review"))[0];
    assert.ok(entry); entry.props.onClick(); assert.deepEqual(f.callbacks, ["metadata"]);
    tree = await f.settle({ ...f.props, subview: "metadata" });
    assert.equal(f.metadata(tree).selectedAlbumId, a.id); assert.deepEqual(f.reads, []);
    f.metadata(tree).onSelectAlbum("A"); tree = await f.settle({ ...f.props, subview: "metadata" });
    assert.equal(f.metadata(tree).selectedAlbumId, "A"); assert.deepEqual(f.reads, []);
    f.metadata(tree).onBack(); assert.deepEqual(f.callbacks, ["metadata", "browser"]);
    tree = await f.settle({ ...f.props, subview: "browser" });
    assert.equal(nodes(tree, n => n.type === "h1")[0].props.children, browseHeading); assert.ok(!f.callbacks.includes("player-back"));
    f.h.cleanup();
  }
});

test("Metadata dialog owners preserve editor identity/state across subviews and reject selection, replacement and stale catalog callbacks", async () => {
  const f = fixture(); let tree = await f.settle(); f.metadata(tree).onSelectAlbum("B"); tree = f.h.render();
  f.metadata(tree).onReview("B"); tree = await f.settle(); const editor = get(tree, Editor);
  assert.equal(editor.props.albumId, "B"); assert.equal(editor.props.reviewContext, null); assert.deepEqual(f.reads, []);
  f.metadata(tree).onSelectAlbum("A"); f.metadata(tree).onReview("A"); f.metadata(tree).onDiagnose("A"); f.metadata(tree).onBack();
  tree = f.h.render(); assert.equal(f.metadata(tree).selectedAlbumId, "B"); assert.equal(get(tree, Editor).props.albumId, "B"); assert.equal(get(tree, Doctor), undefined);
  tree = await f.settle({ ...f.props, subview: "browser" }); assert.equal(get(tree, Editor).props.albumId, "B");
  tree = await f.settle(); assert.equal(get(tree, Editor).props.albumId, "B");
  const prior = get(tree, Editor).props; localAlbumCatalog.state = { ...localAlbumCatalog.getSnapshot(), saveBlocked: true };
  assert.equal(prior.isAvailable(), false); tree = f.h.render(); assert.equal(get(tree, Editor).props.available, false);
  get(tree, Editor).props.onClose(); tree = f.h.render(); assert.equal(get(tree, Editor), undefined);
  const stale = f.metadata(tree); localAlbumCatalog.state = readyCatalog(); stale.onReview("A"); stale.onDiagnose("A");
  assert.equal(get(f.h.render(), Editor), undefined); assert.equal(get(f.h.render(), Doctor), undefined);
  for (const patch of [{ loading: true }, { error: "failed" }, { mpdBusy: true }, { saveBlocked: true }, { albums: [] }]) {
    localAlbumCatalog.state = { ...readyCatalog(), ...patch }; tree = f.h.render(); f.metadata(tree).onReview("B"); f.metadata(tree).onDiagnose("B");
    assert.equal(get(f.h.render(), Editor), undefined); assert.equal(get(f.h.render(), Doctor), undefined);
  }
  f.h.cleanup();
});

test("Metadata Doctor bridge, manual editor close and targeted Apply refresh use the original owners and callbacks", async () => {
  const f = fixture(); let tree = await f.settle(); f.metadata(tree).onSelectAlbum("B"); tree = f.h.render();
  f.metadata(tree).onDiagnose("B"); tree = f.h.render(); const doctor = get(tree, Doctor); assert.equal(doctor.props.albumId, "B");
  f.metadata(tree).onSelectAlbum("A"); assert.equal(f.metadata(f.h.render()).selectedAlbumId, "B");
  const context = { albumId: "B", field: "date", code: "year_divergence" };
  assert.equal(doctor.props.onReview({ ...context, albumId: "A" }), false); assert.equal(doctor.props.onReview(context), true);
  tree = f.h.render(); assert.equal(get(tree, Doctor), undefined); assert.equal(get(tree, Editor).props.reviewContext, context);
  get(tree, Editor).props.onClose(); tree = f.h.render(); assert.equal(f.metadata(tree).selectedAlbumId, "B"); assert.equal(get(tree, Doctor), undefined);
  f.metadata(tree).onReview("B"); tree = f.h.render(); const editor = get(tree, Editor);
  const refreshes = [], refresh = localAlbumCatalog.requestRefresh, wait = localAlbumCatalog.waitForMpd;
  localAlbumCatalog.requestRefresh = () => refreshes.push("refresh"); localAlbumCatalog.waitForMpd = () => refreshes.push("wait");
  try {
    editor.props.onBatchFinished({ tracks: [{ mpd_refresh: { state: "accepted" } }] });
    editor.props.onBatchFinished({ tracks: [{ mpd_refresh: { state: "unavailable" } }] });
    assert.deepEqual(refreshes, ["wait", "refresh"]);
  } finally { localAlbumCatalog.requestRefresh = refresh; localAlbumCatalog.waitForMpd = wait; f.h.cleanup(); }
});

function expressionFrom(file, name) {
  const text = source(file), ast = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX); let expression;
  const visit = node => { if (ts.isVariableDeclaration(node) && node.name.getText(ast) === name) expression = node.initializer.getText(ast); ts.forEachChild(node, visit); };
  visit(ast); assert.ok(expression, name); return compile(`export const extracted = ${expression};`).replace(/^export const extracted = /, "").replace(/;\s*$/, "");
}
function localSidebarClick() {
  const text = source("../App.tsx"), ast = ts.createSourceFile("App.tsx", text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX); let callback;
  const visit = node => {
    if (ts.isJsxSelfClosingElement(node) && node.tagName.getText(ast) === "SidebarNavItem"
      && node.attributes.properties.some(p => p.name?.getText(ast) === "label" && p.initializer.expression.getText(ast).includes("sidebar.local"))) {
      callback = node.attributes.properties.find(p => p.name?.getText(ast) === "onClick").initializer.expression.getText(ast);
    }
    ts.forEachChild(node, visit);
  }; visit(ast); assert.ok(callback); return callback;
}
test("Local sidebar omits Metadata and preserves Local/player subview context and source preference", () => {
  const expression = expressionFrom("../App.tsx", "navigateLocalSubview");
  for (const [player, section, expected] of [[null, "local", [["local", "browser"]]], [{ kind: "local", albumId: "B" }, "plex", [["player", "browser"]]], [null, "plex", [["local", "browser"], ["reset"], ["source", "local"]]]]) {
    const calls = [], before = structuredClone(player);
    const navigate = new Function("playerTarget", "centralSection", "changePlayerLocalSubview", "setLocalSubview", "resetAllNavigation", "mainSectionPreference", `return (${expression});`)(
      player, section, view => calls.push(["player", view]), view => calls.push(["local", view]), () => calls.push(["reset"]), { navigate: view => calls.push(["source", view]) });
    const item = SidebarNavItem({ icon: () => null, label: "Local", active: true, onClick: new Function("navigateLocalSubview", `return (${localSidebarClick()});`)(navigate) });
    item.props.onClick(); assert.deepEqual(calls, expected); assert.deepEqual(player, before);
  }
  const app = source("../App.tsx"); assert.match(app, /<LocalBrowserView\s+subview=\{localSubview\}/); assert.match(app, /subview=\{playerSubview\} onSubviewChange=\{changePlayerLocalSubview\}/);
  assert.ok(app.includes('onClick={() => navigateLocalSubview("browser")}'));
  assert.doesNotMatch(app, /label=\{t\("localMetadata\.title"\)\}/);
  assert.ok(source("../components/LocalBrowserView.tsx").includes('title={t("localMetadata.review")}'));
});

test("App leaves Escape to an open Local dialog instead of unmounting the player host before confirmations", () => {
  const expression = expressionFrom("../App.tsx", "handleKeyDown"), calls = [];
  const doc = { activeElement: { tagName: "BUTTON" }, querySelector: selector => selector === "[data-local-tag-dialog-open]" ? {} : null };
  const handler = new Function("document", "playerNavigationRequests", `return (${expression});`)(doc, { invalidate: () => calls.push("invalidated") });
  handler({ key: "Escape", ctrlKey: false, metaKey: false }); assert.deepEqual(calls, []);
});


test("browser cards retained under Metadata cannot start artwork reads even if previously visible", async () => {
  const f = fixture(); const tree = await f.settle();
  const card = get(tree, VirtualAlbumGrid).props.renderItem(localAlbumCatalog.getSnapshot().albums[0]);
  assert.equal(card.props.readEnabled, false);
  const previousObserver = globalThis.IntersectionObserver;
  globalThis.IntersectionObserver = class { observe() {} disconnect() {} };
  const h = host(card.type, card.props);
  try {
    h.render(); h.effects(); h.slots[2] = true; h.render(); h.effects(); await flush();
    assert.deepEqual(f.reads, []);
  } finally { h.cleanup(); f.h.cleanup(); globalThis.IntersectionObserver = previousObserver; }
});

const presenceAlbum = (id, missing = {}) => {
  const album = sampleAlbum(id);
  return { ...album, tag_presence: { evaluated_track_count: album.track_count, missing: { album: 0, artist: 0, album_artist: 0, date: 0, genre: 0, ...missing } } };
};
const triageAlbums = () => [presenceAlbum("A", { genre: 2 }), presenceAlbum("B", { album_artist: 1, date: 2 }),
  presenceAlbum("C", { album: 1, artist: 3, genre: 4 }), { ...sampleAlbum("legacy"), genres: ["Displayed genre"], year: "2001", artist: "Displayed fallback" }];
function card(tree, label) {
  return nodes(tree, n => n.type === "button" && nodes(n.props.children, child => child.type === "span" && child.props.children === label).length > 0)[0];
}
function fieldCheckbox(tree, label) {
  const node = nodes(tree, n => n.type === "label" && nodes(n.props.children, child => child.type === "span" && child.props.children === label).length > 0)[0];
  return nodes(node, n => n.type === "input" && n.props.type === "checkbox")[0];
}

test("presence triage rejects legacy/incomplete/invalid evidence and never infers tags from displayed values", () => {
  const legacy = { ...sampleAlbum("A"), genres: ["Jazz"], title: "Folder fallback", artist: "Artist fallback", year: "2001" };
  assert.equal(validCatalogTagPresence(legacy), null); assert.equal(catalogPresenceCategory(legacy), "unevaluated");
  for (const invalid of [null, {}, { evaluated_track_count: 0, missing: {} },
    { ...presenceAlbum("A").tag_presence, evaluated_track_count: "3" },
    { ...presenceAlbum("A").tag_presence, evaluated_track_count: 2 },
    { ...presenceAlbum("A").tag_presence, missing: { album: 0, artist: 0, album_artist: 0, date: 0 } },
    ...[-1, 4, 0.5, "0", NaN, Infinity, null, undefined].map(value => ({ ...presenceAlbum("A").tag_presence, missing: { ...presenceAlbum("A").tag_presence.missing, genre: value } })),
  ]) {
    const album = { ...legacy, tag_presence: invalid };
    assert.equal(validCatalogTagPresence(album), null); assert.equal(catalogPresenceCategory(album), "unevaluated");
  }
  assert.equal(catalogPresenceCategory(presenceAlbum("A", { genre: 2 })), "filled", "optional Genre does not classify main fields as missing");
  assert.equal(catalogPresenceCategory(presenceAlbum("A", { album_artist: 1 })), "missing");
  assert.equal(validCatalogTagPresence({ ...presenceAlbum("A"), track_count: 0 }), null);
});

test("presence triage counts affected albums once per field and combines category, search and field filters", () => {
  const albums = triageAlbums(), counts = catalogPresenceCounts(albums);
  assert.deepEqual(counts, { categories: { all: 4, filled: 1, missing: 2, unevaluated: 1 }, fields: { album: 1, artist: 1, album_artist: 1, date: 1, genre: 2 }, evaluated: 3 });
  assert.deepEqual(filterCatalogPresence(albums, "", "missing", ["album_artist", "date"]).map(a => a.id), ["B"]);
  assert.deepEqual(filterCatalogPresence(albums, "", "missing", ["artist", "date"]), []);
  assert.deepEqual(filterCatalogPresence(albums, "2004", "filled", ["genre"]).map(a => a.id), ["A"]);
  assert.deepEqual(filterCatalogPresence(albums, "fallback", "unevaluated", []).map(a => a.id), ["legacy"]);
  assert.deepEqual(filterCatalogPresence(albums, "", "unevaluated", ["genre"]), []);
});

test("presence triage cards/bars/row indicators/track summary and combined filters use catalog evidence in both languages", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__metadataSpaceTranslation = translations[language]; const t = translations[language].t.bind(translations[language]);
    const calls = [], props = { catalog: { ...readyCatalog(), albums: triageAlbums() }, updating: false, selectedAlbumId: "B", dialogsOpen: false,
      onSelectAlbum: id => calls.push(["select", id]), onDiagnose: id => calls.push(["diagnose", id]), onReview: id => calls.push(["review", id]), onBack() {} };
    const h = host(LocalMetadataView, props); let tree = h.render(), markup = renderToStaticMarkup(tree);
    for (const [category, expected] of [["all", 4], ["filled", 1], ["missing", 2], ["unevaluated", 1]]) {
      const c = card(tree, t(`localMetadata.presence.categories.${category}`));
      assert.equal(React.Children.toArray(c.props.children)[1].props.children, expected);
    }
    assert.ok(markup.includes(t("localMetadata.presence.coverage", { evaluated: 3, unknown: 1 })));
    assert.ok(markup.includes(t("localMetadata.presence.overlap")) && markup.includes(t("localMetadata.presence.albumArtistAdvice")));
    assert.ok(markup.includes(t("localMetadata.presence.affectedTracks", { count: 2 })));
    const list = get(tree, VirtualList);
    const optionalRow = renderToStaticMarkup(list.props.renderItem(props.catalog.albums[0]));
    assert.ok(optionalRow.includes(t("localMetadata.presence.categories.filled")) && optionalRow.includes(t("localMetadata.presence.genreOptional")));
    const legacyRow = renderToStaticMarkup(list.props.renderItem(props.catalog.albums[3]));
    assert.match(legacyRow, /<svg/); assert.ok(legacyRow.includes(t("localMetadata.presence.categories.unevaluated")));
    card(tree, t("localMetadata.presence.categories.missing")).props.onClick(); tree = h.render();
    assert.deepEqual(get(tree, VirtualList).props.items.map(a => a.id), ["B", "C"]);
    fieldCheckbox(tree, t("localMetadata.presence.fields.date")).props.onChange({ target: { checked: true } }); tree = h.render();
    assert.deepEqual(get(tree, VirtualList).props.items.map(a => a.id), ["B"]);
    fieldCheckbox(tree, t("localMetadata.presence.fields.artist")).props.onChange({ target: { checked: true } }); tree = h.render();
    assert.deepEqual(get(tree, VirtualList).props.items, []);
    assert.ok(renderToStaticMarkup(tree).includes(t("localMetadata.noMatches")));
    // Filtering does not change the selected identity, nor invoke any action.
    assert.deepEqual(calls, []); button(tree, t("localMetadata.review")).props.onClick(); assert.deepEqual(calls, [["review", "B"]]);
    h.cleanup();
  }
});

test("presence triage reports last snapshot during loading/error/update and never turns missing data into zero pending issues", () => {
  globalThis.__metadataSpaceTranslation = translations.en; const t = translations.en.t.bind(translations.en);
  for (const [patch, updating] of [[{ loading: true }, false], [{ error: "Synthetic error" }, false], [{ mpdBusy: true }, false], [{ saveBlocked: true }, false], [{}, true]]) {
    const h = host(LocalMetadataView, { catalog: { ...readyCatalog(), albums: triageAlbums(), ...patch }, updating, selectedAlbumId: "B", dialogsOpen: false, onSelectAlbum() {}, onDiagnose() {}, onReview() {}, onBack() {} });
    const tree = h.render(), markup = renderToStaticMarkup(tree);
    assert.ok(markup.includes(t("localMetadata.presence.lastSnapshot")));
    assert.equal(React.Children.toArray(card(tree, t("localMetadata.presence.categories.unevaluated")).props.children)[1].props.children, 1);
    assert.equal(button(tree, t("localMetadata.review")).props.disabled, true); h.cleanup();
  }
  const noData = host(LocalMetadataView, { catalog: { ...readyCatalog(), albums: [], loaded: false, loading: true }, updating: false, selectedAlbumId: null, dialogsOpen: false, onSelectAlbum() {}, onDiagnose() {}, onReview() {}, onBack() {} });
  const tree = noData.render(), markup = renderToStaticMarkup(tree);
  assert.ok(markup.includes(t("localMetadata.presence.noSnapshot")));
  for (const category of ["filled", "missing", "unevaluated"]) assert.equal(React.Children.toArray(card(tree, t(`localMetadata.presence.categories.${category}`)).props.children)[1].props.children, "—");
  noData.cleanup();
});

test("presence triage filtering and selecting valid/legacy payloads adds no I/O or catalog instance", async () => {
  const f = fixture(); localAlbumCatalog.state = { ...readyCatalog(), albums: triageAlbums() }; let tree = await f.settle();
  const shared = localAlbumCatalog.getSnapshot(), props = f.metadata(tree), h = host(LocalMetadataView, props);
  let view = h.render(); card(view, translations.en.t("localMetadata.presence.categories.missing")).props.onClick(); view = h.render();
  const row = get(view, VirtualList).props.renderItem(shared.albums[1]); row.props.onClick();
  tree = await f.settle(); assert.equal(f.metadata(tree).selectedAlbumId, "B"); assert.equal(f.metadata(tree).catalog, shared);
  assert.deepEqual(f.reads, []); assert.equal(get(tree, Editor), undefined); assert.equal(get(tree, Doctor), undefined);
  h.cleanup(); f.h.cleanup();
});
