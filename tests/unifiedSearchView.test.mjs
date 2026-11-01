import { navidromeQueueTrack } from "../src/utils/navidromeTracks.ts";
import * as musicSources from "../src/utils/musicSources.ts";
import { passiveQueueInsertion } from "./support/passiveQueueInsertion.mjs";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import { renderToStaticMarkup } from "react-dom/server";
import * as icons from "lucide-react";
import ts from "typescript";
import * as search from "../src/utils/unifiedSearch.ts";
import { confirmedCatalogPlaybackAllowed } from "../src/utils/localAlbumCatalog.ts";
const source = readFileSync(new URL("../src/components/UnifiedSearchView.tsx", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(item => nodes(item, predicate)) : !tree || typeof tree !== "object" ? []
  : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const flush = async () => { for (let i = 0; i < 35; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const empty = { artists: [], albums: [], tracks: [] };
const localAlbum = { id: "local-one", title: "Live", artist: "Local Artist", folder_path: "Source/Live", source_id: "Source", track_count: 1, discs: [] };

function fixture(language = "en", overrides = {}) {
  const dictionary = JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), "utf8"));
  const t = key => key.split(".").reduce((value, part) => value?.[part], dictionary) ?? key;
  let current, timerId = 0;
  const timers = new Map(), events = new Map(), calls = [], errors = [];
  const same = (a, b) => a?.length === b?.length && a.every((value, index) => Object.is(value, b[index]));
  const hooks = {
    useId: () => "unified-keyboard-help",
    useState(initial) { const host = current, index = host.cursor++; host.slots[index] ??= { value: typeof initial === "function" ? initial() : initial }; return [host.slots[index].value, next => { host.slots[index].value = typeof next === "function" ? next(host.slots[index].value) : next; }]; },
    useRef(value) { return current.slots[current.cursor++] ??= { current: value }; },
    useMemo(fn, deps) { const host = current, index = host.cursor++; if (!same(host.slots[index]?.deps, deps)) host.slots[index] = { deps, value: fn() }; return host.slots[index].value; },
    useEffect(fn, deps) { const host = current, index = host.cursor++, old = host.slots[index]; if (!same(old?.deps, deps)) host.effects.push(() => { old?.cleanup?.(); host.slots[index] = { deps, cleanup: fn() }; }); },
    useLayoutEffect(fn, deps) { hooks.useEffect(fn, deps); },
    useSyncExternalStore(_subscribe, get) { return get(); },
  };
  const host = { cursor: 0, slots: [], effects: [],
    catalog: { albums: [localAlbum], loaded: true, loading: false, error: null, saveBlocked: false, mpdBusy: false, ...overrides.catalog },
    props: { config: { local_folders: ["fixture"], plex_token: "fixture", plex_server_id: "server", online_artwork_enabled: false }, plexIdentity: {},
      status: { is_updating: false }, isPlaybackAvailable: true, catalogRefreshRevision: 0, active: true, onFavoritesChanged() {}, ...overrides.props },
    readNav: overrides.readNav ?? (async () => ({connection_id:"fixture-nav",account_id:"a".repeat(64),artists:[],albums:[],songs:[]})),
    navStatus: overrides.navStatus ?? (async () => ({connection_id:"fixture-nav",account_id:"a".repeat(64),server:"http://fixture.invalid",username:"fixture"})),
    readLocal: overrides.readLocal ?? (async () => ({ tracks: [], limited: false })), readPlex: overrides.readPlex ?? (async () => empty),
    paint(props = host.props) { host.props = props; let result; for (let i = 0; i < 2; i++) { host.cursor = 0; host.effects = []; current = host; result = View(host.props); current = null; host.effects.splice(0).forEach(effect => effect()); } host.tree = result; return result; },
    async tick() { const scheduled = [...timers.values()]; timers.clear(); scheduled.forEach(fn => fn()); await flush(); return host.paint(); },
    type(text) { const field = nodes(host.tree, node => node.type === "input")[0]; field.props.onChange({ target: { value: text } }); return host.paint(); },
    close() { host.slots.forEach(slot => slot?.cleanup?.()); },
  };
  const LocalView = () => React.createElement("div", { "data-local-detail": true });
  const AlbumView = props => React.createElement("div", { "data-plex-album-detail": true }, React.createElement("button", { onClick: props.onBack }, props.backLabel ?? "Back to library"));
  const ArtistView = props => React.createElement("div", { "data-plex-artist-detail": true }, React.createElement("button", { onClick: props.onBack }, props.backLabel ?? "Back to library"));
  const dependencies = {...passiveQueueInsertion, react: { __esModule: true, ...React, ...hooks, default: React }, "react/jsx-runtime": jsx,
    "react-i18next": { useTranslation: () => ({ t }) }, "lucide-react": icons, "../utils/unifiedSearch": search,
    "../services/audio": { audioService: { searchLocalTracks: query => { calls.push(["local", query]); return host.readLocal(query); }, playTracks: async (...args) => calls.push(["play", ...args]) } },
    "../services/plex": { plexService: { search: query => { calls.push(["plex", query]); return host.readPlex(query); } } },
    "../utils/localAlbumCatalogSession": { localAlbumCatalog: { subscribe: () => () => {}, getSnapshot: () => host.catalog, ensureLoaded: () => calls.push(["catalog"]) } },
    "../utils/localAlbumCatalog": { confirmedCatalogPlaybackAllowed }, "../utils/frontendDiagnostics": { logError: (...args) => errors.push(args) },
    "../utils/usePlaybackAttempt": { usePlaybackAttempt: () => ({ failure: null, run: action => action({ current: () => true }) }) },
    "./PlaybackRequestNotice": { PlaybackRequestNotice: () => null }, "./LocalBrowserView": { LocalBrowserView: LocalView },
    "./AlbumView": { AlbumView }, "./ArtistView": { ArtistView },
    "../services/navidrome": {navidromeService:{status:()=>{calls.push(["nav-status"]);return host.navStatus();},search:(...args)=>{calls.push(["nav-search",...args]);return host.readNav(...args);}}},
    "../utils/navidromeTracks": {navidromeQueueTrack},
    "./NavidromeView": {NavidromeView: AlbumView}, "./NavidromeArtistView": {NavidromeArtistView: ArtistView},
  };
  const exports = {};
  new Function("require", "exports", "setTimeout", "clearTimeout", "window", "requestAnimationFrame", code)(name => { if(name==="../utils/musicSources")return musicSources; assert.ok(name in dependencies, name); return dependencies[name]; }, exports,
    fn => { const id = ++timerId; timers.set(id, fn); return id; }, id => timers.delete(id),
    { addEventListener: (name, fn) => events.set(name, fn), removeEventListener: name => events.delete(name) }, fn => fn());
  const View = exports.UnifiedSearchView;
  host.paint(); return { host, calls, errors, t, LocalView, AlbumView, ArtistView };
}

test("opening unified Search performs no catalog, network or track reads until a valid query", async () => {
  const { host, calls } = fixture("en", { catalog: { loaded: false, albums: [] } });
  await host.tick(); assert.deepEqual(calls, []); host.type("a"); await host.tick(); assert.deepEqual(calls, []);
  host.type("live"); await host.tick(); assert.deepEqual(calls.map(call => call[0]), ["catalog", "plex"]); host.close();
});
test("cached Local albums appear immediately while track/Plex reads are pending; partial failure keeps Local usable", async () => {
  const tracks = deferred(), plex = deferred();
  const { host, calls, errors, t } = fixture("en", { readLocal: () => tracks.promise, readPlex: () => plex.promise });
  let tree = host.type("Live"); assert.match(renderToStaticMarkup(tree), /Open: Live · Local/); assert.deepEqual(calls, []);
  await host.tick(); assert.deepEqual(calls.map(call => call[0]).sort(), ["local", "plex"]);
  plex.reject(Error("private network diagnostic")); tracks.resolve({ tracks: [], limited: false }); await flush(); tree = host.paint();
  const html = renderToStaticMarkup(tree); assert.match(html, /Open: Live · Local/); assert.ok(html.includes(t("unifiedSearch.partialError")));
  assert.doesNotMatch(html, /private network diagnostic/); assert.equal(errors.length, 1); host.close();
});
test("result navigation retains query and scroll; track clicks alone submit the correct source metadata", async () => {
  const { host, calls, LocalView } = fixture("en", { readLocal: async () => ({ tracks: [{ item_type: "file", path: "Source/Live/image.cue/track0001", name: "Live cue", title: "Live cue", duration: 1801 }], limited: false }) });
  host.type("Live"); let tree = await host.tick(); let focused = 0;
  const viewport = nodes(tree, node => node.props?.ref && node.props?.onScroll)[0]; viewport.props.ref.current = { scrollTop: 0 };
  viewport.props.onScroll({ currentTarget: { scrollTop: 187 } });
  nodes(tree, node => node.type === "button" && node.props["aria-label"] === "Open: Live · Local")[0].props.onClick({ currentTarget: { focus() { focused++; } } });
  tree = host.paint(); const detail = nodes(tree, node => node.type === LocalView)[0]; assert.equal(detail.props.initialAlbumId, localAlbum.id);
  assert.equal(calls.filter(call => call[0] === "play").length, 0);
  tree.props.onKeyDown({ key: "Escape", target: { closest: () => ({}) }, stopPropagation: () => assert.fail("dialog owns Escape") });
  assert.equal(nodes(host.paint(), node => node.type === LocalView).length, 1);
  detail.props.onAlbumNavigationBack(); tree = host.paint(); assert.equal(nodes(tree, node => node.type === "input")[0].props.value, "Live");
  assert.equal(viewport.props.ref.current.scrollTop, 187); assert.equal(focused, 1);
  const track = nodes(tree, node => node.type === "button" && node.props["aria-label"] === "Play track: Live cue · Local")[0];
  track.props.onClick({ currentTarget: {} }); await flush(); const plays = calls.filter(call => call[0] === "play"); assert.equal(plays.length, 1);
  assert.equal(plays[0][1][0].media_locator.uri, "Source/Live/image.cue/track0001"); assert.equal(plays[0][1][0].duration, 1801);
  host.paint({ ...host.props, active: false }); track.props.onClick({ currentTarget: {} }); await flush(); assert.equal(calls.filter(call => call[0] === "play").length, 1); host.close();
});
test("source/type filters avoid irrelevant calls and both languages render without untranslated keys", async () => {
  for (const language of ["en", "pt-BR"]) {
    const { host, calls, t } = fixture(language); host.type("Live");
    nodes(host.tree, node => node.type === "button" && node.props.children === t("explore.sources.local"))[0].props.onClick(); host.paint(); await host.tick();
    assert.deepEqual(calls.map(call => call[0]), ["local"]);
    nodes(host.tree, node => node.type === "button" && node.props.children === t("plex.artists"))[0].props.onClick(); host.paint(); await host.tick();
    assert.deepEqual(calls.map(call => call[0]), ["local"]); assert.doesNotMatch(renderToStaticMarkup(host.tree), /unifiedSearch\.|common\.albumsLabel|common\.artistsLabel/); host.close();
  }
});

test("Plex connection replacement fences the old response and serializes the next query", async () => {
  const old = deferred(), next = deferred(); let reads = 0;
  const { host, calls } = fixture("en", { props: { config: { local_folders: [], plex_token: "fixture", plex_server_id: "server", online_artwork_enabled: false } }, readPlex: () => ++reads === 1 ? old.promise : next.promise });
  host.type("Live"); await host.tick(); assert.equal(reads, 1);
  host.paint({ ...host.props, plexIdentity: {}, config: { ...host.props.config, plex_server_id: "server-b" } });
  await host.tick(); assert.equal(reads, 1);
  old.resolve({ ...empty, albums: [{ rating_key: "old", title: "Old account album", artist: "Old", server_id: "server" }] });
  await flush(); assert.equal(reads, 2); assert.doesNotMatch(renderToStaticMarkup(host.paint()), /Old account album/);
  next.resolve({ ...empty, albums: [{ rating_key: "new", title: "Live new account", artist: "New", server_id: "server-b" }] });
  await flush(); const html = renderToStaticMarkup(host.paint()); assert.match(html, /Live new account/); assert.doesNotMatch(html, /Old account album/);
  assert.deepEqual(calls.map(call => call[0]), ["plex", "plex"]); host.close();
});

test("Local artist and album details survive catalog loading and refreshed rows, but not changed roots", async () => {
  for (const kind of ["artist", "album"]) {
    const { host, LocalView } = fixture();
    host.type(kind === "artist" ? "Local Artist" : "Live");
    const label = `Open: ${kind === "artist" ? "Local Artist" : "Live"} · Local`;
    nodes(host.tree, node => node.type === "button" && node.props["aria-label"] === label)[0].props.onClick({ currentTarget: {} });
    let detail = nodes(host.paint(), node => node.type === LocalView)[0];
    assert.ok(detail);
    const key = detail.key;
    host.catalog = { ...host.catalog, loading: true };
    assert.equal(nodes(host.paint(), node => node.type === LocalView)[0]?.key, key);
    host.catalog = { ...host.catalog, albums: [{ ...localAlbum, track_count: 2 }], loading: false };
    detail = nodes(host.paint(), node => node.type === LocalView)[0];
    assert.equal(detail?.key, key, "routine catalog publication must not unmount the detail");
    assert.equal(detail.props.initialArtist, kind === "artist" ? localAlbum.artist : undefined);
    host.paint({ ...host.props, config: { ...host.props.config, local_folders: ["replacement"] } });
    assert.equal(nodes(host.tree, node => node.type === LocalView).length, 0, "a real source change invalidates the destination");
    host.close();
  }
});

test("opening a Local artist reuses confirmed catalog without pausing artwork; a cold catalog still loads", async () => {
  const { LocalAlbumCatalog } = await import("../src/utils/localAlbumCatalog.ts");
  const { artworkReadBlocked } = await import("../src/utils/localArtwork.ts");
  const path = new URL("../src/components/LocalBrowserView.tsx", import.meta.url);
  const text = readFileSync(path, "utf8");
  const ast = ts.createSourceFile(path.pathname, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let mount;
  const walk = node => {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes("initialCatalogMountHandled.current")) mount = node.arguments[0].getText(ast);
    ts.forEachChild(node, walk);
  };
  walk(ast); assert.ok(mount);
  for (const cold of [false, true]) {
    let reads = 0;
    const catalog = new LocalAlbumCatalog(async () => { reads++; return [localAlbum]; });
    if (!cold) { catalog.ensureLoaded(false, true); await flush(); }
    const env = { initialCatalogMountHandled: { current: false }, initialAlbumId: undefined, initialArtist: "Local Artist",
      metadataMode: false, playerTarget: undefined, isLibraryUpdating: false, isPlaybackAvailable: true, localAlbumCatalog: catalog };
    const js = ts.transpileModule(`const mount = (${mount});`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
    const run = new Function(...Object.keys(env), js + "return mount;")(...Object.values(env));
    run(); run(); // repeated mount setup must not issue another read
    catalog.setUpdating(false); // the independent status effect still runs
    if (!cold) {
      assert.equal(catalog.getSnapshot().loading, false);
      assert.equal(artworkReadBlocked(false, catalog.getSnapshot()), false);
    }
    await flush();
    assert.equal(reads, 1, cold ? "missing catalog loads once" : "confirmed catalog is reused");
    assert.equal(catalog.getSnapshot().loaded, true);
    catalog.setSaveBlocked(true);
    assert.equal(confirmedCatalogPlaybackAllowed(catalog.getSnapshot(), false), false);
    assert.equal(artworkReadBlocked(false, catalog.getSnapshot()), true, "real admission blocks remain in force");
  }
});


// Bind only the owned search controls; no browser, profile or audio process runs.
function bindSearchControls(host) {
  const focused = [], scrolled = [], controls = new Map();
  const field = nodes(host.tree, node => node.type === "input")[0];
  const input = { focus: () => focused.push("input"), select() {}, closest: () => null };
  field.props.ref.current = input;
  for (const row of nodes(host.tree, node => node.props?.["data-search-result"])) {
    const key = row.props["data-search-result"];
    const button = { disabled: Boolean(row.props.disabled), closest: () => null,
      focus: () => focused.push(key), scrollIntoView: options => scrolled.push([key, options]),
      click: () => row.props.onClick({ currentTarget: button }) };
    row.props.ref(button); controls.set(key, button);
  }
  const key = (value, target = input, extra = {}) => {
    let prevented = false;
    host.tree.props.onKeyDown({ key: value, target, preventDefault: () => { prevented = true; }, stopPropagation() {}, ...extra });
    return prevented;
  };
  return { focused, scrolled, controls, input, key };
}

test("search arrows follow displayed category order, Home/End work and upward boundary returns to the input", async () => {
  const { host, calls } = fixture("en", { readPlex: async () => ({ ...empty, albums: [{ rating_key: "remote", title: "Live remote", artist: "Live band", server_id: "server" }] }) });
  host.type("Live"); await host.tick();
  const { focused, scrolled, controls, input, key } = bindSearchControls(host);
  const keys = [...controls.keys()]; assert.equal(keys.length, 2);
  assert.equal(key("ArrowDown"), true); assert.equal(focused.at(-1), keys[0]);
  assert.equal(key("ArrowDown", controls.get(keys[0])), true); assert.equal(focused.at(-1), keys[1]);
  assert.equal(key("ArrowDown", controls.get(keys[1])), true); assert.equal(focused.at(-1), keys[1], "does not wrap unexpectedly");
  key("Home", controls.get(keys[1])); assert.equal(focused.at(-1), keys[0]);
  key("End", controls.get(keys[0])); assert.equal(focused.at(-1), keys[1]);
  key("ArrowUp", controls.get(keys[1])); assert.equal(focused.at(-1), keys[0]);
  key("ArrowUp", controls.get(keys[0])); assert.equal(focused.at(-1), "input");
  key("ArrowUp", input); assert.equal(focused.at(-1), keys.at(-1));
  assert.ok(scrolled.every(([,options]) => options.block === "nearest"));
  assert.equal(key("Enter", controls.get(keys[0])), false, "buttons retain native Enter activation");
  assert.equal(key("Tab", controls.get(keys[0])), false, "Tab remains native");
  assert.equal(calls.filter(call => call[0] === "play").length, 0, "navigation alone never starts playback");
  host.close();
});

test("Enter in the search field opens the first visible match, keeps results on Back, and Escape clears with input focus", async () => {
  const { host, LocalView, calls } = fixture(); host.type("Live"); await host.tick();
  let controls = bindSearchControls(host);
  assert.equal(controls.key("Enter", controls.input, { repeat: true }), true);
  assert.equal(nodes(host.paint(), node => node.type === LocalView).length, 0);
  controls = bindSearchControls(host); assert.equal(controls.key("Enter"), true);
  let tree = host.paint(); assert.equal(nodes(tree, node => node.type === LocalView).length, 1);
  assert.equal(calls.filter(call => call[0] === "play").length, 0);
  assert.equal(controls.key("ArrowDown"), false, "detail owns arrows");
  controls.key("Escape"); tree = host.paint();
  assert.equal(nodes(tree, node => node.type === LocalView).length, 0);
  assert.equal(nodes(tree, node => node.type === "input")[0].props.value, "Live");
  controls = bindSearchControls(host); controls.key("Escape"); tree = host.paint();
  assert.equal(nodes(tree, node => node.type === "input")[0].props.value, "");
  assert.equal(controls.focused.at(-1), "input"); host.close();
});

test("keyboard search respects composition, modifiers, nested dialogs, native controls, filters and disabled playback", async () => {
  const { host, t, calls } = fixture("en", {
    readLocal: async () => ({ tracks: [{ item_type: "file", path: "Source/Live.flac", title: "Live track", name: "Live track" }], limited: false }),
    readPlex: async () => ({ ...empty, tracks: [{ rating_key: "remote-track", title: "Live track", media_locator: { kind: "plex", server_id: "server", part_key: "/fixture" } }] }),
  });
  host.type("Live"); await host.tick(); let bound = bindSearchControls(host);
  for (const extra of [{ nativeEvent: { isComposing: true } }, { ctrlKey: true }, { altKey: true }, { metaKey: true }, { defaultPrevented: true }]) {
    assert.equal(bound.key("ArrowDown", bound.input, extra), false);
    assert.equal(bound.key("Enter", bound.input, extra), false);
    assert.equal(bound.key("Escape", bound.input, extra), false);
  }
  assert.equal(bound.key("Escape", { closest: () => ({}) }), false);
  assert.equal(bound.key("ArrowDown", { closest: () => null }), false, "filter controls retain native keyboard ownership");
  assert.equal(bound.focused.length, 0);
  nodes(host.tree, node => node.type === "button" && node.props.children === t("plex.tracks"))[0].props.onClick(); host.paint();
  bound = bindSearchControls(host); assert.equal(bound.controls.size, 2, "uses only visible filtered rows");
  assert.equal(bound.key("Enter"), true); await flush(); assert.equal(calls.filter(call => call[0] === "play").length, 1);
  host.paint({ ...host.props, isPlaybackAvailable: false }); bound = bindSearchControls(host);
  assert.equal(bound.controls.size, 1); assert.ok([...bound.controls.values()].every(button => button.disabled));
  assert.equal(bound.key("ArrowDown"), false); assert.equal(bound.key("Enter"), false);
  assert.equal(calls.filter(call => call[0] === "play").length, 1);
  host.paint({ ...host.props, active: false }); bound = bindSearchControls(host);
  assert.equal(bound.key("Escape"), false, "hidden search cannot consume keys");
  host.close();
});

test("Plex results are actionable while Local catalog is loading or failed", async () => {
  const plexResult = { ...empty, albums: [{ rating_key: "remote", title: "Live remote", artist: "Artist", server_id: "server" }] };
  for (const error of [null, "unavailable"]) {
    const { host, AlbumView, calls } = fixture("en", { catalog: { loaded: false, albums: [], error }, readPlex: async () => plexResult });
    host.type("Live"); await host.tick();
    const tree = host.paint(); const remote = nodes(tree, node => node.props?.["data-search-result"] === "plex:album:remote")[0];
    assert.ok(remote); assert.equal(remote.props.disabled, false);
    remote.props.onClick({ currentTarget: {} });
    assert.equal(nodes(host.paint(), node => node.type === AlbumView).length, 1);
    assert.equal(calls.filter(call => call[0] === "local").length, 0); host.close();
  }
});

test("a late Plex response does not steal keyboard focus or replace an opened Local destination", async () => {
  const plex = deferred(); const { host, LocalView } = fixture("en", { readPlex: () => plex.promise });
  host.type("Live"); await host.tick(); const { controls, key } = bindSearchControls(host);
  key("Enter"); host.paint(); assert.equal(nodes(host.tree, node => node.type === LocalView).length, 1);
  plex.resolve({ ...empty, albums: [{ rating_key: "first", title: "Live", artist: "First", server_id: "server" }] }); await flush();
  assert.equal(nodes(host.paint(), node => node.type === LocalView).length, 1);
  const current = [...controls.keys()][0]; assert.ok(current.startsWith("local:")); host.close();
});


test("completed Plex matches remain usable while a Local track request is still waiting", async () => {
  const local = deferred(); const { host, AlbumView } = fixture("en", {
    readLocal: () => local.promise,
    readPlex: async () => ({ ...empty, albums: [{ rating_key: "remote", title: "Live remote", artist: "Artist", server_id: "server" }] }),
  });
  host.type("Live"); await host.tick();
  const row = nodes(host.tree, node => node.props?.["data-search-result"] === "plex:album:remote")[0];
  assert.ok(row); row.props.onClick({ currentTarget: {} });
  assert.equal(nodes(host.paint(), node => node.type === AlbumView).length, 1);
  local.resolve({ tracks: [], limited: false }); await flush();
  assert.equal(nodes(host.paint(), node => node.type === AlbumView).length, 1); host.close();
});

test('Plex album and artist opened from unified search each expose one Back to search and retain query/focus on return', async()=>{
 for(const language of ['en','pt-BR']) {
  const plexResult={artists:[{rating_key:'artist',name:'Live Artist',server_id:'server'}],albums:[{rating_key:'album',title:'Live Album',artist:'Artist',server_id:'server'}],tracks:[]};
  const f=fixture(language,{readPlex:async()=>plexResult});f.host.type('Live');let tree=await f.host.tick();const viewport=nodes(tree,n=>n.props?.ref&&n.props?.onScroll)[0];viewport.props.ref.current={scrollTop:0};
  for(const kind of ['album','artist']) {
   const label=kind==='album'?'Live Album':'Live Artist';let focused=0;
   const button=nodes(tree,n=>n.type==='button'&&n.props['aria-label']?.includes(label)&&n.props['aria-label']?.includes('Plex'))[0];assert.ok(button);button.props.onClick({currentTarget:{focus(){focused++}}});tree=f.host.paint();
   const html=renderToStaticMarkup(tree);const text=f.t('unifiedSearch.back');assert.equal(html.split(text).length-1,1);assert.doesNotMatch(html,/Back to library|Voltar para a biblioteca/);
   const detail=nodes(tree,n=>n.type===(kind==='album'?f.AlbumView:f.ArtistView))[0];assert.equal(detail.props.backLabel,text);detail.props.onBack();tree=f.host.paint();assert.equal(nodes(tree,n=>n.type==='input')[0].props.value,'Live');assert.equal(focused,1);
  }
  f.host.close();
 }
});


const navConnection={connection_id:"fixture-nav",account_id:"a".repeat(64)};
const navResults={...navConnection,albums:[{id:"nav-album",title:"Live Nav",artist:"Nav Artist"}],artists:[{id:"nav-artist",name:"Nav Artist",album_count:1}],songs:[{id:"nav-song",title:"Live Nav Song",artist:"Nav Artist",album:"Live Nav",duration:120,disc:1,track:1,album_id:"nav-album"}]};
test("Navidrome search is lazy, source-filtered and keeps typed album/artist destinations and track playback",async()=>{
 const {host,calls,t,AlbumView,ArtistView}=fixture("en",{props:{navidromeConfigured:true},readNav:async()=>navResults});
 await host.tick();assert.deepEqual(calls,[]);host.type("Live");
 nodes(host.tree,n=>n.type==="button"&&n.props.children===t("explore.sources.navidrome"))[0].props.onClick();host.paint();let tree=await host.tick();
 assert.deepEqual(calls.map(c=>c[0]),["nav-status","nav-search"]);
 for(const [label,View]of [["Live Nav",AlbumView],["Nav Artist",ArtistView]]){
  nodes(tree,n=>n.type==="button"&&n.props["aria-label"]===`Open: ${label} · Navidrome`)[0].props.onClick({currentTarget:{}});
  const detail=nodes(host.paint(),n=>n.type===View)[0];assert.ok(detail);
  assert.equal((detail.props.initialAlbum??detail.props.artist).account_id,navConnection.account_id);
  assert.equal(detail.props.backLabel,t("unifiedSearch.back"));
  (detail.props.onAlbumBack??detail.props.onBack)();tree=host.paint();assert.equal(nodes(tree,n=>n.type==="input")[0].props.value,"Live");
 }
 assert.equal(calls.some(c=>c[0]==="play"),false);
 nodes(tree,n=>n.type==="button"&&n.props["aria-label"]==="Play track: Live Nav Song · Navidrome")[0].props.onClick({currentTarget:{}});await flush();
 assert.deepEqual(calls.find(c=>c[0]==="play")[1][0].media_locator,{kind:"navidrome",...navConnection,song_id:"nav-song"});host.close();
});
test("a Navidrome failure preserves other source results, while disabled or unconfigured Navidrome never reads",async()=>{
 const f=fixture("en",{props:{navidromeConfigured:true},readNav:async()=>{throw Error("PRIVATE account detail");}});
 f.host.type("Live");await f.host.tick();const html=renderToStaticMarkup(f.host.tree);assert.match(html,/Open: Live · Local/);assert.doesNotMatch(html,/PRIVATE/);assert.equal(f.errors.length,1);f.host.close();
 for(const props of [{navidromeConfigured:false},{navidromeConfigured:true,config:{local_folders:[],plex_token:"",sources:{local:false,plex:false,navidrome:false}}}]){
  const f=fixture("en",{props});f.host.type("Live");await f.host.tick();assert.equal(f.calls.some(c=>c[0].startsWith("nav-")),false);f.host.close();
 }
});
test("Navidrome account revision invalidates pending and opened results without touching the other source lanes",async()=>{
 const old=deferred();let reads=0;
 const f=fixture("en",{props:{navidromeConfigured:true},readNav:async()=>++reads===1?old.promise:{...navResults,albums:[{...navResults.albums[0],title:"Live new account"}]}});
 f.host.type("Live");await f.host.tick();f.host.paint({...f.host.props,navidromeRevision:1});await f.host.tick();assert.equal(reads,1);
 old.resolve(navResults);await flush();let tree=f.host.paint();assert.equal(reads,2);assert.doesNotMatch(renderToStaticMarkup(tree),/Open: Live Nav · Navidrome/);assert.match(renderToStaticMarkup(tree),/Live new account/);
 nodes(tree,n=>n.type==="button"&&n.props["aria-label"]==="Open: Live new account · Navidrome")[0].props.onClick({currentTarget:{}});assert.ok(nodes(f.host.paint(),n=>n.type===f.AlbumView).length);
 f.host.paint({...f.host.props,navidromeRevision:2});assert.equal(nodes(f.host.tree,n=>n.type===f.AlbumView).length,0);
 f.host.paint({...f.host.props,config:{...f.host.props.config,sources:{local:true,plex:true,navidrome:false}}});assert.doesNotMatch(renderToStaticMarkup(f.host.tree),/Live new account/);f.host.close();
});
