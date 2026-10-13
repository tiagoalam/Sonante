import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { createExploreSession, deriveExploreGroups, EXPLORE_PAGE_SIZE, EXPLORE_SUGGESTION_COUNT, exploreEntryIsCurrent } from "./explore.ts";
import { exploreGenreCardStyle } from "./exploreGenreCards.ts";
import { PlexArtistPortraitCache } from "./plexArtistPortraits.ts";
import { LocalAlbumCatalog } from "./localAlbumCatalog.ts";
import { LocalArtworkResolver } from "./localArtwork.ts";
import { visitPlayerTarget } from "./playerNavigation.ts";
const local = (id, artist = id) => ({ id, title: "Same title", artist, folder_path: `Synthetic/${id}`, discs: [], track_count: 1 });
const plex = (id, artist = id) => ({ rating_key: id, title: "Same title", artist, thumb: { server_id: "server", path: `/covers/${id}` } });
const context = (n = 8) => ({ local: { albums: Array.from({ length: n }, (_, i) => local(`L${i}`)), state: "ready" },
  plex: { albums: Array.from({ length: n }, (_, i) => plex(`P${i}`)), state: "ready", identity: {}, serverId: "server", libraryId: "library" } });
const flush = async () => { for (let i = 0; i < 16; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const source = path => readFileSync(new URL(path, import.meta.url), "utf8");
const dataModule = text => `data:text/javascript;base64,${Buffer.from(text).toString("base64")}`;
const compile = text => ts.transpileModule(text, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;

test("opt-in Explore snapshots distinguish inventory, sampling and slices without repeated-render logs or changing selection", async () => {
  const helper = dataModule(compile(source("./frontendDiagnostics.ts").replaceAll("import.meta.env", '{"DEV":true,"VITE_SONANTE_EXPLORE_DIAGNOSTICS":"1"}')));
  const instrumented = await import(dataModule(compile(source("./explore.ts").replace(/from "(\.[^"]+)"/g, (_, path) => `from "${path === "./frontendDiagnostics.ts" ? helper : new URL(path, import.meta.url).href}"`))));
  const old = console.info, logs = [];
  console.info = (label, json) => { assert.equal(label, "[Sonante] explore.diagnostic"); logs.push(JSON.parse(json)); };
  try {
    const c = catalogWithSlices();
    c.local.albums.forEach((a, i) => { a.source_id = i < 10 ? "/private/root-one" : "/private/root-two"; });
    c.local.albums[0].title = "https://private/?token=secret";
    const s = instrumented.createExploreSession(() => 0.5), control = createExploreSession(() => 0.5);
    s.syncContext(c, true); control.syncContext(c, true);
    assert.deepEqual(s.getSnapshot(), control.getSnapshot());
    assert.deepEqual(logs[0].albums.received, { local: 15, plex: 3 });
    assert.deepEqual(logs[0].albums.eligible, logs[0].albums.received);
    assert.equal(logs[0].albums.selected.local + logs[0].albums.selected.plex, 5);
    assert.deepEqual(logs[0].artists, { derived: 2, eligible: 2, sampled: 2, visible: 2 });
    assert.deepEqual(Object.values(logs[0].localRoots.received).sort((a,b)=>a-b), [5, 10]);
    for (let i=0;i<20;i++) s.syncContext({ local: { ...c.local }, plex: { ...c.plex } }, true);
    assert.equal(logs.length, 1);
    s.renew(); control.renew(); assert.deepEqual(s.getSnapshot(), control.getSnapshot());
    s.openSlice(s.getSnapshot().genres[0], s.getSnapshot().revision);
    assert.deepEqual(logs.at(-1).slice.eligible, { local: 15, plex: 3 });
    assert.equal(logs.at(-1).slice.pages, 3); assert.equal(logs.at(-1).slice.sample.length, 3);
    s.setPage(1, s.getSnapshot().revision); assert.equal(logs.at(-1).event, "page");
    const library = logs[0].plexLibrary;
    s.syncContext({ ...c, plex: { ...c.plex, libraryId: "private-other-library", identity: {} } }, true);
    assert.notEqual(logs.at(-1).plexLibrary, library); assert.equal(logs.at(-1).slice, null);
    s.syncContext({ ...c, local: { ...c.local, state: "unavailable" } }, true);
    assert.equal(logs.at(-1).albums.eligible.local, 0); assert.equal(logs.at(-1).albums.visible.local, 0);
    const diverse = context(50);
    [...diverse.local.albums, ...diverse.plex.albums].forEach((a, i) => { a.genres = [`Genre ${i % 20}`]; });
    s.syncContext(diverse, true); s.renew();
    assert.equal(logs.at(-1).artists.derived, 100); assert.equal(logs.at(-1).artists.sampled, 4);
    assert.equal(logs.at(-1).genres.derived, 20); assert.equal(logs.at(-1).genres.sampled, 4);
    assert.doesNotMatch(JSON.stringify(logs), /private|secret|https:|folder_path|rating_key|source_id/);
  } finally { console.info = old; }
});

// The actual session uses injected randomness only in these synthetic tests.
test("ten suggestions retain exact homonymous editions, artist diversity and both sources without excluding incomplete metadata", () => {
  const c = context(), session = createExploreSession(() => 0.4); session.syncContext(c, true);
  const selected = session.getSnapshot().selected;
  assert.equal(selected.length, 10); assert.equal(new Set(selected.map(e => e.album.artist)).size, 10);
  assert.equal(new Set(selected.map(e => e.source)).size, 2); assert.equal(new Set(selected.map(e => e.key)).size, 10);
  for (const entry of selected) assert.ok(c[entry.source].albums.includes(entry.album));
  const sparse = { ...c, local: { state: "ready", albums: [local("a", ""), local("b", "")] }, plex: { ...c.plex, albums: [plex("a", "")] } };
  const s = createExploreSession(() => 0); s.syncContext(sparse, true);
  assert.equal(s.getSnapshot().selected.length, 3); assert.equal(new Set(s.getSnapshot().selected.map(e => e.key)).size, 3);
  const repeated = context(); repeated.local.albums.forEach(a => a.artist = "Repeated");
  const r = createExploreSession(() => 0.3); r.syncContext(repeated, true);
  assert.ok(r.getSnapshot().selected.some(e => e.source === "local"));
  assert.ok(r.getSnapshot().selected.filter(e => e.source === "plex").length >= 4, "diverse artists take priority after both sources are represented");
});

test("selection is stable under rerenders/detail return; renewal is in memory and changes selection when alternatives exist", () => {
  const c = context(), s = createExploreSession(() => 0.5); s.syncContext(c, true); const before = s.getSnapshot();
  for (let i = 0; i < 20; i++) s.syncContext({ local: { ...c.local }, plex: { ...c.plex } }, true);
  assert.equal(s.getSnapshot(), before);
  s.syncContext(c, false); assert.deepEqual(s.getSnapshot().selected.map(e => e.key), before.selected.map(e => e.key));
  s.syncContext(c, true); assert.deepEqual(s.getSnapshot().selected.map(e => e.key), before.selected.map(e => e.key));
  s.renew(); assert.notDeepEqual(s.getSnapshot().selected.map(e => e.key), before.selected.map(e => e.key));
  assert.equal(c.local.albums.length, 8); assert.equal(c.plex.albums.length, 8);
});

test("initial provisional Local data waits for its confirmed response; later sources preserve every published card", () => {
  const c = context(), s = createExploreSession(() => 0.5);
  const provisional = { ...c, local: { albums: [local("provisional")], state: "loading" }, plex: { ...c.plex, state: "loading" } };
  s.syncContext(provisional, true);
  assert.equal(s.getSnapshot().preparing, true); assert.deepEqual(s.visibleEntries(), []);
  assert.deepEqual(s.visibleArtists(), []); assert.deepEqual(s.visibleGenres(), []);
  s.renew(); assert.deepEqual(s.visibleEntries(), []);
  s.syncContext(c, true);
  assert.equal(s.getSnapshot().preparing, false); assert.equal(s.visibleEntries().length, 10);
  assert.ok(s.visibleEntries().every(e => e.album.id !== "provisional"));
  const unavailable = createExploreSession(() => 0.5);
  unavailable.syncContext({ ...c, local: { ...c.local, state: "unavailable" } }, true);
  const first = unavailable.visibleEntries().map(e => e.key);
  unavailable.syncContext(c, true); assert.deepEqual(unavailable.visibleEntries().map(e => e.key), first);
  unavailable.renew(); assert.ok(unavailable.visibleEntries().some(e => e.source === "local"));
});

test("ready Local publishes without waiting for Plex, including pending/failure, empty and invalid contexts", () => {
  for (const phase of ["loading", "unavailable"]) {
    const c = context(); c.plex = { ...c.plex, albums: [], state: phase };
    const s = createExploreSession(() => 0.5); s.syncContext(c, true);
    assert.equal(s.getSnapshot().preparing, false); assert.equal(s.visibleEntries().length, 8);
    assert.ok(s.visibleEntries().every(e => e.source === "local"));
    const before = s.getSnapshot();
    const next = { ...c, plex: { ...c.plex, albums: context().plex.albums, state: "ready" } };
    s.syncContext(next, true);
    assert.deepEqual(s.visibleEntries().map(e => e.key), before.selected.map(e => e.key));
    assert.deepEqual(s.visibleArtists().map(e => e.key), before.artists.map(e => e.key));
    s.syncContext(next, false); s.syncContext(next, true);
    assert.equal(s.getSnapshot().preparing, false);
    assert.deepEqual(s.visibleEntries().map(e => e.key), before.selected.map(e => e.key));
    s.renew(); assert.ok(s.visibleEntries().some(e => e.source === "plex"));
    const old = s.visibleEntries().find(e => e.source === "plex");
    s.syncContext({ ...next, plex: { ...c.plex, identity: {}, serverId: "other", state: "loading" } }, true);
    assert.equal(s.validDetail(old), false); assert.ok(s.visibleEntries().every(e => e.source === "local"));
  }
  const c = context(0), s = createExploreSession();
  c.local.state = "empty"; c.plex.state = "loading"; s.syncContext(c, true);
  assert.equal(s.getSnapshot().preparing, true);
  s.syncContext({ ...c, plex: { ...c.plex, state: "unavailable", loading: true } }, true);
  assert.equal(s.getSnapshot().preparing, true, "one failed library does not settle another pending library");
  s.syncContext({ ...c, plex: { ...c.plex, state: "unavailable", loading: false } }, true);
  assert.equal(s.getSnapshot().preparing, false); assert.deepEqual(s.visibleEntries(), []);
  s.syncContext({ ...c, plex: { ...c.plex, state: "empty" } }, true);
  assert.equal(s.getSnapshot().preparing, false); assert.deepEqual(s.visibleEntries(), []);
});

test("a short published shelf does not fill or reshuffle on late arrival; renewal sees the expanded catalog", () => {
  const c = context(2); c.plex = { ...c.plex, albums: [], state: "loading" };
  const s = createExploreSession(() => 0.5); s.syncContext(c, true); const first = s.visibleEntries().map(e => e.key);
  const next = { ...c, plex: { ...c.plex, albums: context().plex.albums, state: "ready" } };
  s.syncContext(next, true); assert.deepEqual(s.visibleEntries().map(e => e.key), first);
  s.renew(); assert.equal(s.visibleEntries().length, 10);
});

test("stale click tokens fail across inventory, scope, credential identity, visibility, availability and closure without opening another edition", () => {
  for (const cause of ["localVersion", "plexVersion", "identity", "library", "visibility", "availability", "close"]) {
    const c = context(), s = createExploreSession(() => 0.5); s.syncContext(c, true);
    const old = s.getSnapshot(), entry = old.selected.find(e => e.source === (cause === "localVersion" ? "local" : "plex"));
    assert.equal(s.canUse(entry, old.revision), true);
    let next = c;
    if (cause === "localVersion") next = { ...c, local: { ...c.local, albums: c.local.albums.map(a => ({ ...a })) } };
    if (cause === "plexVersion") next = { ...c, plex: { ...c.plex, albums: c.plex.albums.map(a => ({ ...a })) } };
    if (cause === "identity") next = { ...c, plex: { ...c.plex, identity: {}, serverId: "other-server" } };
    if (cause === "library") next = { ...c, plex: { ...c.plex, identity: {}, libraryId: "other-library" } };
    if (cause === "availability") next = { ...c, plex: { ...c.plex, state: "unavailable" } };
    s.syncContext(next, cause !== "visibility"); if (cause === "close") s.close();
    assert.equal(s.canUse(entry, old.revision), false, cause);
    if (["localVersion", "plexVersion", "identity", "library", "availability"].includes(cause)) assert.equal(s.validDetail(entry), false, cause);
  }
});

test("shared Local coordinator initializes once and blocks loading under update, save, error or MPD unavailability", async () => {
  for (const [updating, healthy] of [[true, true], [false, false]]) {
    let calls = 0; const catalog = new LocalAlbumCatalog(async () => { calls++; return []; }); catalog.ensureLoaded(updating, healthy); await flush(); assert.equal(calls, 0);
  }
  const pending = deferred(); let calls = 0; const catalog = new LocalAlbumCatalog(() => { calls++; return pending.promise; });
  catalog.setSaveBlocked(true); catalog.ensureLoaded(false, true); await flush(); assert.equal(calls, 0);
  catalog.state = { ...catalog.getSnapshot(), saveBlocked: false };
  catalog.ensureLoaded(false, true); catalog.ensureLoaded(false, true); await flush(); assert.equal(calls, 1);
  pending.resolve([local("one")]); await flush(); for (let i = 0; i < 10; i++) catalog.ensureLoaded(false, true);
  assert.equal(calls, 1); assert.equal(catalog.getSnapshot().loaded, true);
  const failed = new LocalAlbumCatalog(async () => { throw Error("synthetic"); }); failed.ensureLoaded(false, true); await flush();
  failed.ensureLoaded(false, true); assert.equal(failed.getSnapshot().error, "synthetic");
});

test("guarded local-only artwork never consults online and drops invalidated replies before caching or reading other discs", async () => {
  const pending = deferred(), reads = []; let online = 0, blocked = false;
  const resolver = new LocalArtworkResolver(path => { reads.push(path); return pending.promise; }, async () => { online++; return "online"; });
  const album = { ...local("one"), discs: [{ folder_path: "Synthetic/disc2" }] };
  const load = resolver.resolveLocalOnly(album, () => blocked); blocked = true; pending.resolve("stale");
  assert.equal(await load, null); assert.equal(resolver.get(album.id), null); assert.equal(online, 0); assert.equal(reads.length, 1);
  const miss = new LocalArtworkResolver(async () => null, async () => { online++; return "online"; });
  assert.equal(await miss.resolveLocalOnly(album), null); assert.equal(online, 0);
});

const translations = {};
for (const language of ["pt-BR", "en"]) {
  const instance = i18next.createInstance(); await instance.init({ lng: language, resources: { [language]: { translation: JSON.parse(source(`../locales/${language}.json`)) } }, interpolation: { escapeValue: false } }); translations[language] = instance;
}
const hooksUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))}; export default React;
 ${["useState", "useRef", "useMemo", "useEffect", "useLayoutEffect", "useSyncExternalStore"].map(name => `export const ${name} = (...args) => globalThis.__exploreHost.${name}(...args);`).join("\n")}`);
const translationUrl = dataModule('export const useTranslation = () => ({t: (...args) => globalThis.__exploreTranslation.t(...args)});');
const artworkUrl = dataModule('export const localArtworkResolver = { get: id => globalThis.__exploreResolver?.get(id) ?? globalThis.__exploreCached?.(id) ?? null, subscribe: (id, fn) => globalThis.__exploreResolver?.subscribe(id, fn) ?? (() => {}), resolve: (album, online, blocked, phase, interested) => globalThis.__exploreResolver ? globalThis.__exploreResolver.resolve(album, online, blocked, phase, interested) : globalThis.__exploreCover(album, () => blocked() || !interested()) };');
const plexImageUrl = dataModule('export const PlexImage = () => null;');
const placeholderUrl = dataModule('export const AlbumArtworkPlaceholder = () => null;');
const componentUrl = dataModule(compile(source("../components/ExploreView.tsx"))
  .replace('from "react"', `from ${JSON.stringify(hooksUrl)}`)
  .replace(/from "(react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/plexArtistPortraits"', `from ${JSON.stringify(new URL("./plexArtistPortraits.ts", import.meta.url).href)}`)
  .replace('from "../utils/exploreGenreCards"', `from ${JSON.stringify(new URL("./exploreGenreCards.ts", import.meta.url).href)}`)
  .replace('from "../utils/exploreAlbumShelf"', `from ${JSON.stringify(new URL("./exploreAlbumShelf.ts", import.meta.url).href)}`)
  .replace('from "../utils/explore"', `from ${JSON.stringify(new URL("./explore.ts", import.meta.url).href)}`)
  .replace('from "../utils/localArtworkSession"', `from ${JSON.stringify(artworkUrl)}`)
  .replace('from "./PlexImage"', `from ${JSON.stringify(plexImageUrl)}`)
  .replace('from "./AlbumArtworkPlaceholder"', `from ${JSON.stringify(placeholderUrl)}`));
const { ExploreView, ExploreAlbumCard, ExploreArtistPortrait, ExploreAlbumShelf } = await import(componentUrl);
function host(Component, props) {
  let cursor = 0; const slots = [], effects = [], children = new Map();
  const h = {
    useState(initial) { const i = cursor++; if (!(i in slots)) slots[i] = typeof initial === "function" ? initial() : initial; return [slots[i], v => slots[i] = typeof v === "function" ? v(slots[i]) : v]; },
    useRef(initial) { return slots[cursor++] ??= { current: initial }; },
    useMemo(fn, deps) { const i = cursor++; if (!slots[i] || deps.some((v,j) => v !== slots[i].deps[j])) slots[i] = {deps,value:fn()}; return slots[i].value; },
    useEffect(fn,deps) { const i = cursor++, old = slots[i]; if (!old || deps.some((v,j)=>v!==old.deps[j])) effects.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()};}); },
    useLayoutEffect(fn,deps) { h.useEffect(fn,deps); },
    useSyncExternalStore(_,get) { return get(); },
    render(next=props) {
      props=next;cursor=0;globalThis.__exploreHost=h;const tree=Component(props);
      const used=new Set();
      const expand=node=>{
        if(Array.isArray(node))return node.map(expand);
        if(!React.isValidElement(node))return node;
        if(node.type===ExploreAlbumShelf){
          const id=node.props.id;used.add(id);let child=children.get(id);
          if(!child){child=host(ExploreAlbumShelf,node.props);children.set(id,child);}return child.render(node.props);
        }
        return React.cloneElement(node,{},expand(node.props.children));
      };
      const result=Component===ExploreView?expand(tree):tree;
      if(Component===ExploreView)for(const [id,child]of children)if(!used.has(id)){child.close();children.delete(id);}
      return result;
    },
    effects() { effects.splice(0).forEach(fn=>fn());children.forEach(child=>child.effects()); }, close() {slots.forEach(s=>s?.cleanup?.());children.forEach(child=>child.close());},slots,
  }; return h;
}
function nodes(tree,predicate) { if(Array.isArray(tree))return tree.flatMap(n=>nodes(n,predicate));if(!tree||typeof tree!=="object")return[];return[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)]; }
function shallowMarkup(tree) {
  if(Array.isArray(tree))return tree.map((child,index)=>{
    const node=shallowMarkup(child);
    return React.isValidElement(node)&&node.key===null?React.cloneElement(node,{key:index}):node;
  });
  if(!React.isValidElement(tree))return tree;
  if(tree.type===ExploreArtistPortrait)return React.createElement("div",{key:tree.key});
  if(tree.type===ExploreAlbumCard)return React.createElement("article",{key:tree.key},tree.props.entry.album.title);
  return React.cloneElement(tree,{},shallowMarkup(tree.props.children));
}

test("real Explore shelf shows independent source states and selected library, without exposing errors or credentials", () => {
  for(const language of ["pt-BR","en"])for(const phase of ["loading","empty","not_configured","unavailable","ready"]){
    globalThis.__exploreTranslation=translations[language];const c=context();c.local.state=phase;
    const h=host(ExploreView,{context:c,active:true,libraryTitle:"Selected library",ensureLocal(){},isCurrent:()=>true,onOpen(){}});
    const tree=h.render(),markup=renderToStaticMarkup(shallowMarkup(tree));
    if(phase === "ready" || phase === "not_configured") assert.ok(!markup.includes(translations[language].t(`explore.states.${phase}`)));
    else assert.ok(markup.includes(translations[language].t(`explore.states.${phase}`)));
    assert.ok(markup.includes(translations[language].t(phase === "not_configured" ? "explore.plexScope" : "explore.aggregateScope")));
    assert.ok(nodes(tree,n=>n.type===ExploreAlbumCard).some(n=>n.props.entry.source==="plex"));
    assert.ok(!markup.includes(translations[language].t("explore.creatingSuggestions")));h.close();
  }
});

test("actual shelf card opens its exact entry, returns to the same selection, renews without queries and rejects stale handlers", () => {
  globalThis.__exploreTranslation=translations.en;const c=context(),opened=[];
  const props={context:c,active:true,libraryTitle:"Selected",ensureLocal(){},isCurrent:()=>true,onOpen:e=>opened.push(e)};
  const h=host(ExploreView,props);let tree=h.render();const before=nodes(tree,n=>n.type===ExploreAlbumCard),entry=before[0];
  const card=host(ExploreAlbumCard,entry.props);const button=card.render();button.props.onClick();assert.equal(opened[0],entry.props.entry);
  h.render({...props,active:false});button.props.onClick();assert.equal(opened.length,1);
  tree=h.render(props);assert.deepEqual(nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key),before.map(n=>n.props.entry.key));
  const renew=nodes(tree,n=>n.type==="button")[0];renew.props.onClick();tree=h.render();assert.notDeepEqual(nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key),before.map(n=>n.props.entry.key));
  h.close();button.props.onClick();assert.equal(opened.length,1);card.close();
});

test("actual local card reads only its displayed album and discards delayed cover after context invalidation or closure", async () => {
  for(const closed of [false,true]){
    globalThis.__exploreTranslation=translations.en;const c=context(),s=createExploreSession(()=>0.5);s.syncContext(c,true);
    const entry=s.getSnapshot().selected.find(e=>e.source==="local"),revision=s.getSnapshot().revision,pending=deferred(),calls=[];
    globalThis.__exploreCover=(album,blocked)=>{calls.push({album,blocked});return pending.promise;};
    const h=host(ExploreAlbumCard,{entry,revision,active:true,current:()=>s.canUse(entry,revision),onOpen(){}});h.render();h.effects();
    assert.equal(calls.length,1);assert.equal(calls[0].album,entry.album);
    if(closed)h.close();else s.syncContext({...c,local:{...c.local,albums:c.local.albums.map(a=>({...a}))}},true);
    assert.equal(calls[0].blocked(),true);pending.resolve("stale");await flush();assert.equal(h.slots[0],null);h.close();
  }
});

// Source-coupled extraction exercises App's actual expressions/effects with simulated service responses.
const appSource=source("../App.tsx"),appAst=ts.createSourceFile("App.tsx",appSource,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
function expression(name){let out;const visit=n=>{if(ts.isVariableDeclaration(n)&&n.name.getText(appAst)===name)out=n.initializer.getText(appAst);ts.forEachChild(n,visit);};visit(appAst);assert.ok(out,name);return compile(`export const extracted=${out};`).replace(/^export const extracted\s*=\s*/,"").replace(/;\s*$/,"");}
function effect(fragment){let out;const visit=n=>{if(ts.isCallExpression(n)&&n.expression.getText(appAst)==="useEffect"&&n.arguments[0].getText(appAst).includes(fragment))out=n.arguments[0].getText(appAst);ts.forEachChild(n,visit);};visit(appAst);assert.ok(out,fragment);return compile(`export const extracted=${out};`).replace(/^export const extracted\s*=\s*/,"").replace(/;\s*$/,"");}

test("App source classification distinguishes not configured/loading/empty/error and masks obsolete Plex scope", () => {
  const classifyLocal=new Function("localExploreConfigured","isPlaybackAvailable","playbackStatus","localCatalog","mpdHealth",`return (${expression("localExploreState")});`);
  const localState=(configured,available,status,catalog,health={state:available?"available":"unavailable"})=>classifyLocal(configured,available,status,catalog,health);
  const base={loaded:true,loading:false,saveBlocked:false,mpdBusy:false,error:null,albums:[local("one")]};
  assert.equal(localState(false,true,{is_updating:false},base),"not_configured");assert.equal(localState(true,false,{is_updating:false},base),"unavailable");
  assert.equal(localState(true,true,{is_updating:true},base),"unavailable");assert.equal(localState(true,true,{is_updating:false},{...base,loaded:false}),"loading");
  assert.equal(localState(true,true,{is_updating:false},{...base,albums:[]}),"empty");assert.equal(localState(true,true,{is_updating:false},{...base,error:"secret error"}),"unavailable");
  assert.equal(localState(true,false,{is_updating:false},base,{state:"starting"}),"loading");
  assert.equal(localState(true,true,{is_updating:false},{...base,loading:true}),"ready", "confirmed catalog remains usable during revalidation");
  const plexState=new Function("isPlexConnected","aggregateCurrent","plexLibrariesIdentity","plexConnectionIdentity","loadingLibraries","aggregateAlbums","plexLibrariesError","aggregateFailures","aggregateLoading","plexAggregate","libraries",`return (${expression("plexExploreState")});`);
  const conn={},args=[true,true,conn,conn,false,[plex("one")],false,[],false,{started:true},[{key:"library"}]];
  assert.equal(plexState(...args),"ready");
  for(const [index,value,expected]of[[0,false,"not_configured"],[1,false,"loading"],[2,{},"loading"],[4,true,"loading"],[5,[],"empty"]]){const changed=[...args];changed[index]=value;assert.equal(plexState(...changed),expected);}
  assert.equal(plexState(...args.slice(0,5),[],true,[],false,{started:true},[]),"unavailable");
  assert.equal(plexState(...args.slice(0,5),[],false,["Failed"],false,{started:true},[]),"unavailable");
  assert.equal(plexState(...args.slice(0,5),[],false,[],true,{started:true},[]),"loading");
  assert.equal(plexState(...args.slice(0,5),[plex("one")],false,["Failed"],true,{started:true},[]),"ready");

});

test("existing Plex album query publishes error or data once and drops disposed/generation-obsolete responses without another query", async () => {
  for(const scenario of ["success","error","disposed","generation"]){
    const calls=[],pending=deferred(),status=[],published=[],identity={},generation={current:0},conn={};
    const setters=["setAlbums","setCollections","setHasCollections","setLoading","setActiveAlbum","setActiveCollection","setActiveArtist","setFallbackPlexArtist","setSelectedPlexDecade","setSelectedPlexYear","setSelectedPlexUnknownYear","setSelectedPlexGenre","setSearchQuery","setSearchResults","setPlexViewMode"];
    const values=setters.map(name=>name==="setAlbums"?v=>published.push(v):()=>{});
    const run=new Function("selectedLibrary","plexLibrariesIdentity","plexConnectionIdentity","explorePlexIdentity","plexLibraryRequestGenerationRef","setPlexAlbumLoad","plexService","plexCatalog","plexAlbumLoad","logError",...setters,`return (${effect("plexCatalog.loadLibrary(selectedLibrary")});`)(
      {key:"exact-library"},conn,conn,identity,generation,v=>status.push(v),{getCollections:()=>Promise.resolve([])},{loadLibrary:(...a)=>{calls.push(a);return pending.promise;}},null,()=>{},...values);
    const cleanup=run();assert.deepEqual(calls,[[{key:"exact-library"}]]);assert.equal(status[0].phase,"loading");
    if(scenario==="disposed")cleanup();if(scenario==="generation")generation.current++;
    if(scenario==="error")pending.reject(Error("DO_NOT_EXPOSE"));else pending.resolve([plex("exact")]);await flush();
    assert.equal(calls.length,1);assert.equal(status.at(-1).phase,scenario==="success"?"ready":scenario==="error"?"error":"loading");
    if(scenario==="success")assert.equal(published.at(-1)[0].rating_key,"exact");else assert.deepEqual(published,[]);cleanup();
  }
});

test("existing Plex discovery captures library failures and discards responses after scope disposal", async () => {
  for (const scenario of ["success", "error", "disposed"]) {
    const pending=deferred(), identity={}, errors=[], libraries=[], loading=[], selected=[]; let calls=0;
    const setters=["setAlbums","setCollections","setActiveAlbum","setActiveCollection","setActiveArtist","setFallbackPlexArtist","setSelectedPlexDecade","setSelectedPlexYear","setSelectedPlexUnknownYear","setSelectedPlexGenre"];
    const run=new Function("config","mainSectionPreference","setLoadingLibraries","setLibraries","setSelectedLibrary","setPlexLibrariesError","setPlexLibrariesIdentity","plexConnectionIdentity","plexService","logError",...setters,`return (${effect(".getLibraries()")});`)(
      {plex_token:"SYNTHETIC"},{plexDiscoveryFinished(){}},v=>loading.push(v),v=>libraries.push(v),v=>selected.push(v),v=>errors.push(v),v=>assert.equal(v,identity),identity,{getLibraries(){calls++;return pending.promise;}},()=>{},...setters.map(()=>()=>{}));
    const cleanup=run(); assert.equal(calls,1); assert.equal(loading.at(-1),true);
    if(scenario==="disposed")cleanup();
    if(scenario==="error")pending.reject(Error("DO_NOT_EXPOSE"));else pending.resolve([{key:"exact",title:"Library"}]);
    await flush(); assert.equal(calls,1);
    assert.equal(errors.at(-1),scenario==="error");
    if(scenario==="success")assert.equal(selected.at(-1).key,"exact");
    else assert.deepEqual(libraries,[[]]);
    assert.equal(loading.at(-1),scenario==="disposed");cleanup();
  }
});

test("App's actual open/back handlers reject stale identities and keep last-section state unchanged", () => {
  const c=context(),s=createExploreSession(()=>0.5);s.syncContext(c,true);const entry=s.getSnapshot().selected[0];
  const live={current:{context:c,visible:true}},changes=[];
  const current=new Function("exploreLive","exploreEntryIsCurrent",`return (${expression("exploreEntryCurrent")});`)(live,exploreEntryIsCurrent);
  const open=new Function("exploreLive","exploreEntryCurrent","setExploreDetail","setExploreLocalSubview",`return (${expression("openExploreAlbum")});`)(live,current,e=>changes.push(e),v=>assert.equal(v,"browser"));
  open(entry);assert.equal(changes[0],entry);live.current.visible=false;open(entry);assert.equal(changes.length,1);
  live.current={context:{...c,local:{...c.local,albums:[]},plex:{...c.plex,identity:{}}},visible:true};open(entry);assert.equal(changes.length,1);
  const close=new Function("document","setExploreDetail",`return (${expression("closeExploreAlbum")});`)({querySelector:()=>null},e=>changes.push(e));close();assert.equal(changes.at(-1),null);
  assert.match(appSource,/ExploreView portraits=\{artistPortraits\} context=\{exploreContext\}/);assert.match(appSource,/initialAlbumId=\{exploreDetail\.album\.id\}/);
  assert.match(appSource,/album=\{exploreDetail\.album\} onBack=\{closeExploreAlbum\}/);
  assert.match(appSource,/onlineArtworkEnabled=\{config\?\.online_artwork_enabled \?\? false\}/);
  assert.match(appSource,/subview=\{exploreLocalSubview\} onSubviewChange=\{setExploreLocalSubview\}/);
  const guardedClose=new Function("document","setExploreDetail",`return (${expression("closeExploreAlbum")});`)({querySelector:()=>({})},e=>changes.push(e));
  const count=changes.length;guardedClose();assert.equal(changes.length,count);
});

test("player visits preserve Explore as the return section without replacing its shelf or anchored detail", () => {
  const c=context(),s=createExploreSession(()=>0.5);s.syncContext(c,true);const before=s.getSnapshot().selected;
  const localTarget={kind:"local",view:"album",albumId:"exact",verified:{catalog:[],groups:[]}};
  const plexTarget={kind:"plex",view:"album",album:plex("exact"),artist:{rating_key:"artist",name:"Artist"}};
  let visit=visitPlayerTarget(null,localTarget,"explore");s.syncContext(c,false);
  visit=visitPlayerTarget(visit,plexTarget,"plex");assert.equal(visit.returnSection,"explore");
  s.syncContext(c,true);assert.deepEqual(s.getSnapshot().selected,before);
  assert.match(appSource,/const centralSection = playerReturnSection \?\? mediaSource/);
  assert.match(appSource,/else if \(playerTarget\) \{\s*closePlayerNavigation\(\);\s*\} else if \(exploreDetail\)/);
});

// Directed increment tests use only synthetic catalog payloads and the existing hook host.
test("artist groups preserve Local/Plex origins, Plex rating identities and scoped nominal fallback", () => {
  const c=context(0);
  c.local.albums=[{...local("one"," Artist "),source_id:"source-one"},{...local("two","artist"),source_id:"source-two"},local("unknown","")];
  c.plex.albums=[{...plex("edition-one","Artist"),artist_rating_key:"100"},{...plex("edition-two","Artist"),artist_rating_key:"200"},plex("fallback","Artist")];
  const groups=deriveExploreGroups(c);assert.equal(groups.artists.length,4);
  assert.equal(new Set(groups.artists.map(group=>group.key)).size,4);
  const s=createExploreSession(()=>0.5);s.syncContext(c,true);
  for(const group of s.getSnapshot().artists){
    s.openSlice(group,s.getSnapshot().revision);const entries=s.getSnapshot().slice.entries;
    if(group.source==="local"){
      assert.deepEqual(entries.map(e=>e.album.id),["one","two"]);
      assert.deepEqual(entries.map(e=>e.album.source_id),["source-one","source-two"]);
    }else {
      assert.equal(entries.length,1);assert.equal(entries[0].source,"plex");
      assert.equal(entries[0].identity,c.plex.identity);
      const expected={"rating:100":"edition-one","rating:200":"edition-two","name:artist":"fallback"};
      assert.equal(entries[0].album.rating_key,expected[group.artistId]);
    }
    assert.ok(entries.every(e=>c[e.source].albums.includes(e.album)));
    s.leaveSlice(s.getSnapshot().revision);
  }
  const other=deriveExploreGroups({...c,plex:{...c.plex,libraryId:"other"}});
  assert.notDeepEqual(other.artists.filter(g=>g.source==="plex").map(g=>g.key),groups.artists.filter(g=>g.source==="plex").map(g=>g.key));
});

test("genres reuse validated normalization, filter both sources and never remove untagged albums from shelves", () => {
  const c=context(0);
  c.local.albums=[{...local("one"),genres:[" ROCK  ","rock"," ","bad\u0000genre"]},local("absent"),{...local("empty"),genres:[]}];
  c.plex.albums=[{...plex("one","Other artist"),genres:["Rock","Jazz"]},...Array.from({length:3},(_,i)=>({...plex(`extra-${i}`),genres:["Rock"]}))];
  const groups=deriveExploreGroups(c);assert.deepEqual(groups.genres.map(g=>g.value),["jazz","rock"]);
  const rock=groups.genres.find(g=>g.value==="rock");assert.deepEqual(rock.sources,["local","plex"]);
  const s=createExploreSession(()=>0.4);s.syncContext(c,true);assert.equal(s.getSnapshot().selected.length,7);
  assert.ok(s.visibleEntries().some(e=>e.source==="local"&&e.album.id==="absent"));
  s.openSlice(s.getSnapshot().genres.find(g=>g.value==="rock"),s.getSnapshot().revision);
  const filtered=s.getSnapshot().slice.entries;assert.equal(filtered.length,5);
  assert.equal(new Set(filtered.map(e=>e.key)).size,5);assert.deepEqual(filtered.map(e=>e.source),["local","plex","plex","plex","plex"]);
  assert.ok(filtered.every(e=>e.album.title==="Same title"));
});

test("group derivation and all shelves stay stable through telemetry and detail return; renewal only samples cached inventory", () => {
  const c=context(20);let genreReads=0;
  [...c.local.albums,...c.plex.albums].forEach((a,i)=>Object.defineProperty(a,"genres",{get(){genreReads++;return [`Genre ${i%8}`];}}));
  const s=createExploreSession(()=>0.4);s.syncContext(c,true);const initial=s.getSnapshot(),reads=genreReads;
  for(let i=0;i<30;i++){s.syncContext({local:{...c.local},plex:{...c.plex}},true);s.candidateCount();s.visibleArtists();s.visibleGenres();}
  assert.equal(s.getSnapshot(),initial);assert.equal(genreReads,reads);
  s.syncContext(c,false);s.syncContext(c,true);
  for(const field of ["selected","artists","genres"])assert.deepEqual(s.getSnapshot()[field],initial[field]);
  assert.equal(genreReads,reads);
  s.renew();const renewed=s.getSnapshot();
  for(const field of ["selected","artists","genres"])assert.notDeepEqual(renewed[field].map(x=>x.key),initial[field].map(x=>x.key));
  assert.equal(genreReads,reads);assert.equal(renewed.selected.length,10);assert.equal(renewed.artists.length,4);assert.equal(renewed.genres.length,4);
  assert.equal(new Set(renewed.artists.map(g=>g.source)).size,2);
});

test("slice membership has its own page guard, never relaxes shelf canUse and restores page after detail", () => {
  const c=context(0);c.local.albums=Array.from({length:19},(_,i)=>({...local(String(i),"One artist"),genres:["One genre"]}));
  const s=createExploreSession(()=>0.5);s.syncContext(c,true);const initial=s.getSnapshot();
  s.openSlice(initial.artists[0],initial.revision);s.setPage(1,s.getSnapshot().revision);
  const before=s.getSnapshot(),page=s.sliceEntries();assert.equal(page.length,EXPLORE_PAGE_SIZE);
  const outside=page.find(e=>!initial.selected.some(old=>old.key===e.key));assert.ok(outside);
  assert.equal(s.canUse(outside,before.revision),false);assert.equal(s.canUseSlice(outside,before.revision),true);
  assert.equal(s.canUseSlice(before.slice.entries[0],before.revision),false);
  s.syncContext(c,false);assert.equal(s.canUseSlice(outside,before.revision),false);
  s.syncContext(c,true);assert.equal(s.getSnapshot().slice,before.slice);assert.deepEqual(s.sliceEntries(),page);
  assert.equal(s.canUseSlice(outside,s.getSnapshot().revision),true);
  const unchanged=s.getSnapshot();s.setPage(100,unchanged.revision);s.setPage(-1,unchanged.revision);s.setPage(0.5,unchanged.revision);assert.equal(s.getSnapshot(),unchanged);
  s.leaveSlice(s.getSnapshot().revision);assert.equal(s.getSnapshot().slice,null);assert.deepEqual(s.getSnapshot().selected,initial.selected);
});

test("catalog/scope/availability changes invalidate slices, old group buttons, result clicks and page commands", () => {
  for(const cause of ["local","plex","credential","server","library","unavailable","source"]){
    const c=context(0);c.local.albums=[{...local("one"),genres:["Rock"]},...Array.from({length:3},(_,i)=>({...local(`extra-${i}`),genres:["Rock"]}))];c.plex.albums=[{...plex("one","Plex artist"),genres:["Rock"]}];
    const s=createExploreSession(()=>0.5);s.syncContext(c,true);const group=s.getSnapshot().genres[0],groupRevision=s.getSnapshot().revision;
    s.openSlice(group,groupRevision);const old=s.getSnapshot(),entry=s.sliceEntries()[0];assert.equal(s.canUseSlice(entry,old.revision),true);
    let next=c;
    if(cause==="local")next={...c,local:{...c.local,albums:c.local.albums.map(a=>({...a}))}};
    if(cause==="plex")next={...c,plex:{...c.plex,albums:c.plex.albums.map(a=>({...a}))}};
    if(cause==="credential")next={...c,plex:{...c.plex,identity:{}}};
    if(cause==="server")next={...c,plex:{...c.plex,serverId:"other"}};
    if(cause==="library")next={...c,plex:{...c.plex,libraryId:"other"}};
    if(cause==="unavailable")next={...c,local:{...c.local,state:"unavailable"}};
    if(cause==="source")next={...c,local:{...c.local,state:"not_configured"}};
    s.syncContext(next,true);assert.equal(s.getSnapshot().slice,null,cause);assert.equal(s.getSnapshot().sliceInvalidated,true);
    assert.equal(s.canUseSlice(entry,old.revision),false);const current=s.getSnapshot();s.openSlice(group,groupRevision);s.setPage(0,old.revision);assert.equal(s.getSnapshot(),current);
  }
  const c=context(),s=createExploreSession(()=>0.5);s.syncContext(c,true);const old=s.getSnapshot();s.close();s.openSlice(old.artists[0],old.revision);assert.equal(s.getSnapshot().slice,null);
});

function catalogWithSlices(){
  const c=context(0);c.local.albums=Array.from({length:15},(_,i)=>({...local(`L${i}`,"Artist"),title:i<4?`Other title ${i}`:"Same title",genres:["Rock"]}));
  c.plex.albums=Array.from({length:3},(_,i)=>({...plex(`P${i}`,"Artist"),artist_rating_key:"100",genres:["rock"]}));return c;
}
const namedButton=(tree,name)=>nodes(tree,n=>n.type==="button"&&(React.Children.toArray(n.props.children).includes(name)||nodes(n,c=>c.type==="p"&&c.props.children===name).length))[0];
test("real artist/genre buttons show bounded pages, open exact albums and return to slice then unchanged shelves without catalog or metadata I/O", () => {
  for(const language of ["pt-BR","en"])for(const kind of ["artist","genre"]){
    globalThis.__exploreTranslation=translations[language];let ensured=0,covers=0;const opened=[],c=catalogWithSlices(),t=translations[language].t.bind(translations[language]);
    globalThis.__exploreCover=async()=>{covers++;return null;};
    const props={context:c,active:true,libraryTitle:"Exact library",ensureLocal(){ensured++;},isCurrent:()=>true,onOpen:e=>opened.push(e)};
    const h=host(ExploreView,props);let tree=h.render();h.effects();
    const shelves=nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key);
    const groupName=kind==="artist"?"Artist":"Rock";
    const groupButton=kind==="artist"?nodes(tree,n=>n.type==="button"&&nodes(n,p=>p.type==="p"&&p.props.children==="Artist").length&&nodes(n,p=>p.type==="p"&&typeof p.props.children==="string"&&p.props.children.startsWith(t("explore.sources.local")+" · ")).length)[0]:namedButton(tree,groupName);
    assert.ok(groupButton);groupButton.props.onClick();
    tree=h.render();h.effects();assert.equal(ensured,1);assert.equal(covers,0);assert.equal(opened.length,0);
    let cards=nodes(tree,n=>n.type===ExploreAlbumCard);assert.equal(cards.length,6);
    namedButton(tree,t("explore.nextPage")).props.onClick();tree=h.render();h.effects();cards=nodes(tree,n=>n.type===ExploreAlbumCard);assert.equal(cards.length,6);
    const page=cards.map(n=>n.props.entry.key),card=host(ExploreAlbumCard,cards[0].props);card.render().props.onClick();assert.equal(opened[0],cards[0].props.entry);
    h.render({...props,active:false});h.effects();tree=h.render(props);h.effects();
    assert.deepEqual(nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key),page);
    namedButton(tree,t("explore.backToShelves")).props.onClick();tree=h.render();h.effects();
    assert.deepEqual(nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key),shelves);
    assert.equal(ensured,2,"coordinator called on initial activation and detail return only; never on group/page selection or hide");
    assert.equal(covers,0,"this host does not mount child artwork effects; selection itself starts no artwork I/O");
    h.close();card.close();
  }
});

test("real slice invalidates delayed cover and obsolete UI controls without reopening or routing another edition", async () => {
  globalThis.__exploreTranslation=translations.en;const c=catalogWithSlices(),opened=[],pending=deferred();let blocked;
  globalThis.__exploreCover=(album,guard)=>{blocked=guard;return pending.promise;};
  const props={context:c,active:true,libraryTitle:"Library",ensureLocal(){},isCurrent:()=>true,onOpen:e=>opened.push(e)};
  const h=host(ExploreView,props);let tree=h.render();h.effects();
  const group=nodes(tree,n=>n.type==="button"&&nodes(n,p=>p.type==="p"&&p.props.children==="Artist").length&&nodes(n,p=>p.type==="p"&&typeof p.props.children==="string"&&p.props.children.startsWith(translations.en.t("explore.sources.local")+" · ")).length)[0];
  group.props.onClick();
  tree=h.render();h.effects();const back=namedButton(tree,translations.en.t("explore.backToShelves")),next=namedButton(tree,translations.en.t("explore.nextPage"));
  const node=nodes(tree,n=>n.type===ExploreAlbumCard)[0],card=host(ExploreAlbumCard,node.props),button=card.render();card.effects();assert.equal(blocked(),false);
  tree=h.render({...props,context:{...c,local:{...c.local,albums:c.local.albums.map(a=>({...a}))}}});h.effects();
  assert.equal(blocked(),true);pending.resolve("stale");await flush();assert.equal(card.slots[0],null);
  button.props.onClick();group.props.onClick();next.props.onClick();back.props.onClick();assert.equal(opened.length,0);
  tree=h.render();assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(translations.en.t("explore.sliceInvalidated")));
  assert.equal(namedButton(tree,translations.en.t("explore.backToShelves")),undefined);h.close();card.close();
});

test("real aggregate shelf opens another library without browser navigation and renders translated partial progress/retry", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__exploreTranslation=translations[language];const t=translations[language].t.bind(translations[language]);
    const c=context(0);c.local.state="empty";
    const a=Array.from({length:7},(_,i)=>({...plex(`A${i}`,"One artist"),title:`Album A${i}`,artist_rating_key:"100",genres:["Rock"],library_id:"1",library_title:"One"}));
    const b=Array.from({length:7},(_,i)=>({...plex(`B${i}`,"Other artist"),title:`Album B${i}`,artist_rating_key:"200",genres:["Rock"],library_id:"2",library_title:"Two"}));
    c.plex={...c.plex,albums:[...a,...b],libraryId:"",catalogs:[{libraryId:"1",albums:a},{libraryId:"2",albums:b}],completeness:"partial",loading:true};
    const live={current:{context:c,visible:true}},opened=[],selectedLibrary={key:"1"};let retries=0;
    const current=new Function("exploreLive","exploreEntryIsCurrent",`return (${expression("exploreEntryCurrent")});`)(live,exploreEntryIsCurrent);
    const open=new Function("exploreLive","exploreEntryCurrent","setExploreDetail","setExploreLocalSubview",`return (${expression("openExploreAlbum")});`)(live,current,e=>opened.push(e),()=>{});
    const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:current,onOpen:open,
      plexProgress:{loading:true,failedLibraries:["Unavailable library"],discoveryFailed:false,complete:false},retryPlex(){retries++;}};
    const h=host(ExploreView,props);let tree=h.render();h.effects();
    const text=renderToStaticMarkup(shallowMarkup(tree));assert.ok(text.includes(t("explore.aggregateScope")));
    assert.ok(text.includes(t("explore.librariesLoading")));assert.ok(text.includes("Unavailable library"));
    namedButton(tree,t("explore.retryLibraries")).props.onClick();assert.equal(retries,1);
    const group=namedButton(tree,"Other artist");assert.ok(group);group.props.onClick();tree=h.render();h.effects();
    const cards=nodes(tree,n=>n.type===ExploreAlbumCard);assert.equal(cards.length,6);assert.ok(cards.every(n=>n.props.entry.libraryId==="2"));
    const card=host(ExploreAlbumCard,cards[0].props);card.render().props.onClick();assert.equal(opened[0],cards[0].props.entry);
    assert.equal(opened[0].album.rating_key,cards[0].props.entry.album.rating_key);assert.equal(selectedLibrary.key,"1");
    namedButton(tree,t("explore.backToShelves")).props.onClick();tree=h.render();namedButton(tree,"Rock").props.onClick();tree=h.render();
    assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(t("explore.sliceCount",{count:14})));
    namedButton(tree,t("explore.nextPage")).props.onClick();tree=h.render();assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,6);
    h.close();card.close();
  }
});

test("actual shelf transitions from preparation to definitive cards, with late load and detail return preserving entries", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__exploreTranslation = translations[language]; const c = context();
    const props = { context: { ...c, local: { ...c.local, state: "loading" }, plex: { ...c.plex, state: "loading" } }, active: true, libraryTitle: null, ensureLocal(){}, isCurrent:()=>true, onOpen(){} };
    const h = host(ExploreView, props); let tree = h.render(); h.effects();
    assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,0);
    assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(translations[language].t("explore.creatingSuggestions")));
    const ready = { ...props, context: { ...c, plex: { ...c.plex, albums: [], state: "loading" } } };
    tree = h.render(ready); h.effects(); const keys = nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key);
    assert.equal(keys.length,8); assert.ok(!renderToStaticMarkup(shallowMarkup(tree)).includes(translations[language].t("explore.creatingSuggestions")));
    tree = h.render({ ...ready, context: c }); h.effects();
    assert.deepEqual(nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key),keys);
    h.render({ ...ready, context:c, active:false }); tree=h.render({ ...ready,context:c });
    assert.deepEqual(nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key),keys); h.close();
  }
});

test("cards resolve explicit source display names, expose the full caption and use honest fallback without internal IDs/paths", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__exploreTranslation=translations[language]; const c=context(),s=createExploreSession(()=>0.5);s.syncContext(c,true);
    for (const entry of s.visibleEntries().slice(0,2)) {
      entry.album.source_id="opaque-internal"; entry.album.source_label="Configured Local library"; entry.album.library_title="Configured Plex library";
      const props={entry,revision:s.getSnapshot().revision,active:true,current:()=>true,onOpen(){}};
      const h=host(ExploreAlbumCard,props),tree=h.render();
      const name=entry.source==="local"?entry.album.source_label:entry.album.library_title;
      const caption=`${translations[language].t(`explore.sources.${entry.source}`)} · ${name}`;
      assert.ok(nodes(tree,n=>n.type==="p"&&n.props.title===caption).some(n=>n.props.children===caption));
      if(entry.source==="local")delete entry.album.source_label;else entry.album.library_title=" ";
      const fallback=`${translations[language].t(`explore.sources.${entry.source}`)} · ${translations[language].t("explore.unknownOrigin")}`;
      assert.ok(nodes(h.render(),n=>n.type==="p"&&n.props.title===fallback).length);
      assert.doesNotMatch(renderToStaticMarkup(h.render()),/opaque-internal|Synthetic|undefined/); h.close();
    }
  }
});

test("a loaded Local cover survives a session revision/late library without another artwork request", async () => {
  globalThis.__exploreTranslation=translations.en;
  const c=context(),s=createExploreSession(()=>0.5);s.syncContext(c,true);
  let entry=s.visibleEntries().find(e=>e.source==="local"),revision=s.getSnapshot().revision,reads=0;
  globalThis.__exploreCover=async()=>{reads++;return "data:image/png;base64,synthetic";};
  const props={entry,revision,active:true,current:()=>s.canUse(entry,revision),onOpen(){}};
  const h=host(ExploreAlbumCard,props);h.render();h.effects();await flush();assert.equal(reads,1);
  const cover=h.slots[0];s.syncContext({...c,plex:{...c.plex,loading:true}},true);revision=s.getSnapshot().revision;
  entry=s.visibleEntries().find(e=>e.key===entry.key);
  h.render({...props,entry,revision,current:()=>s.canUse(entry,revision)});h.effects();await flush();
  assert.equal(h.slots[0],cover);assert.equal(reads,1);
  globalThis.__exploreCached=()=>cover;
  s.syncContext({...c,local:{...c.local,albums:c.local.albums.map(a=>({...a}))}},true);
  revision=s.getSnapshot().revision;entry=s.visibleEntries().find(e=>e.key===entry.key);
  h.render({...props,entry,revision,current:()=>s.canUse(entry,revision)});h.effects();
  assert.equal(h.slots[0],cover);assert.equal(reads,1);delete globalThis.__exploreCached;h.close();
});


test("portrait arrival changes only the image; same-name Local stays unassociated and navigation keeps the library/page", async () => {
  globalThis.__exploreTranslation=translations.en;
  const c=catalogWithSlices();c.plex.albums.forEach(a=>a.library_title="Other library");
  const pending=deferred(), calls=[];
  const cache=new PlexArtistPortraitCache((server,key)=>{calls.push([server,key]);return pending.promise;});
  cache.setContext(c.plex.identity,c.plex.serverId);
  const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){},portraits:cache};
  const h=host(ExploreView,props);let tree=h.render();h.effects();
  const before=h.slots[0].value.getSnapshot();
  const portraits=nodes(tree,n=>n.type===ExploreArtistPortrait);
  assert.equal(portraits.length,2);
  const plexNode=portraits.find(n=>n.props.group.source==="plex"), localNode=portraits.find(n=>n.props.group.source==="local");
  const localPhoto=host(ExploreArtistPortrait,localNode.props);localPhoto.render();localPhoto.effects();
  const photo=host(ExploreArtistPortrait,plexNode.props);let image=photo.render();photo.effects();await flush();
  assert.deepEqual(calls,[["server","100"]]);assert.equal(nodes(image,n=>n.props?.image).length,0);
  pending.resolve({server_id:"server",path:"/library/metadata/100/thumb/1"});await flush();
  image=photo.render();const picture=nodes(image,n=>n.props?.image)[0];
  assert.equal(picture.props.image.path,"/library/metadata/100/thumb/1");assert.equal(picture.props.cacheScope,c.plex.identity);
  assert.equal(nodes(localPhoto.render(),n=>n.props?.image).length,0);
  tree=h.render();assert.equal(h.slots[0].value.getSnapshot(),before);
  const button=nodes(tree,n=>n.type==="button"&&nodes(n,p=>p.type===ExploreArtistPortrait&&p.props.group.source==="plex").length)[0];
  button.props.onClick();tree=h.render();assert.equal(h.slots[0].value.getSnapshot().slice.group.libraryId,"library");
  assert.equal(h.slots[0].value.getSnapshot().slice.entries.length,3);assert.equal(h.slots[0].value.getSnapshot().slice.page,0);
  assert.equal(calls.length,1);h.close();photo.close();localPhoto.close();cache.invalidate();
});

test("portrait component discards obsolete artist/auth replies and reuses a cached portrait on return", async () => {
  globalThis.__exploreTranslation=translations.en;
  const identity={}, pending=deferred(), calls=[];
  const cache=new PlexArtistPortraitCache((server,key)=>{calls.push(key);return key==="one"?pending.promise:Promise.resolve({server_id:server,path:`/library/metadata/${key}/thumb/1`});});
  cache.setContext(identity,"server");
  const group={kind:"artist",key:"one",source:"plex",artistId:"rating:one",ratingKey:"one",name:"Same name"};
  const props={group,identity,serverId:"server",active:true,cache};const h=host(ExploreArtistPortrait,props);
  h.render();h.effects();await flush();
  const next={...props,group:{...group,key:"two",ratingKey:"two"}};
  assert.equal(nodes(h.render(next),n=>n.props?.image).length,0);h.effects();await flush();
  pending.resolve({server_id:"server",path:"/library/metadata/one/thumb/1"});await flush();
  assert.equal(nodes(h.render(),n=>n.props?.image)[0].props.image.path,"/library/metadata/two/thumb/1");
  h.render({...next,active:false});h.effects();assert.equal(nodes(h.render(next),n=>n.props?.image).length,1);h.effects();await flush();assert.equal(calls.length,2);
  const newIdentity={};cache.setContext(newIdentity,"server");
  assert.equal(nodes(h.render({...next,identity:newIdentity}),n=>n.props?.image).length,0);
  h.effects();await flush();assert.equal(calls.length,3);h.close();cache.invalidate();
});


test("actual album shelf uses its measured area, bounds arrows/keyboard and preserves position through details and late catalogs", () => {
  globalThis.__exploreTranslation=translations.en;
  const previous={window:globalThis.window,ResizeObserver:globalThis.ResizeObserver};
  let resized,reduce=false;globalThis.window={matchMedia:()=>({matches:reduce})};
  globalThis.ResizeObserver=class { constructor(callback){resized=callback;} observe(){} disconnect(){} };
  const c=context(14),listeners=new Map(),moves=[];
  const node={clientWidth:1000,scrollLeft:0,cardWidth:190,count:10,
    get scrollWidth(){return this.count*this.cardWidth+(this.count-1)*24;},
    addEventListener(event,fn){listeners.set(event,fn);},removeEventListener(event,fn){if(listeners.get(event)===fn)listeners.delete(event);},
    scrollTo(options){moves.push(options);this.scrollLeft=Math.max(0,Math.min(options.left,this.scrollWidth-this.clientWidth));listeners.get("scroll")?.();}};
  const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}};
  const h=host(ExploreView,props);let current=props,tree;
  const render=(next=current)=>{
    current=next;
    for(let i=0;i<3;i++){
      tree=h.render(current);const viewport=nodes(tree,n=>n.props?.id==="explore-album-shelf")[0];
      if(viewport){viewport.props.ref.current=node;node.cardWidth=parseFloat(viewport.props.style.gridAutoColumns);node.count=viewport.props.children.length;}
      h.effects();
    }
    return tree;
  };
  const arrow=key=>nodes(tree,n=>n.type==="button"&&n.props["aria-label"]===translations.en.t(key))[0];
  const keys=()=>nodes(tree,n=>n.type===ExploreAlbumCard).map(n=>n.props.entry.key);
  try {
    render();const first=keys(),snapshot=h.slots[0].value.getSnapshot();
    assert.equal(first.length,10);assert.equal(new Set(first).size,10);assert.equal(node.cardWidth,180.8);
    assert.equal(arrow("explore.previousSuggestions").props.disabled,true);assert.equal(arrow("explore.nextSuggestions").props.disabled,false);
    arrow("explore.nextSuggestions").props.onClick();render();const end=node.scrollLeft;
    assert.equal(end,node.scrollWidth-node.clientWidth);assert.equal(moves.at(-1).behavior,"smooth");
    assert.equal(arrow("explore.nextSuggestions").props.disabled,true);assert.equal(arrow("explore.previousSuggestions").props.disabled,false);
    assert.equal(h.slots[0].value.getSnapshot(),snapshot,"scrolling must not sample or invalidate the shelf");
    const viewport=nodes(tree,n=>n.props?.id==="explore-album-shelf")[0];let prevented=false;
    viewport.props.onKeyDown({key:"Home",target:node,currentTarget:node,preventDefault(){prevented=true;}});render();assert.equal(prevented,true);assert.equal(node.scrollLeft,0);
    reduce=true;nodes(tree,n=>n.props?.id==="explore-album-shelf")[0].props.onKeyDown({key:"End",target:node,currentTarget:node,preventDefault(){}});render();
    assert.equal(node.scrollLeft,end);assert.equal(moves.at(-1).behavior,"auto");
    const called=moves.length;nodes(tree,n=>n.props?.id==="explore-album-shelf")[0].props.onKeyDown({key:"ArrowLeft",target:{},currentTarget:node,preventDefault(){assert.fail("card key handling must remain native");}});assert.equal(moves.length,called);
    h.render({...current,active:false});node.clientWidth=0;node.scrollLeft=0;listeners.get("scroll")?.();h.effects();
    node.clientWidth=1000;render();assert.equal(node.scrollLeft,end);assert.deepEqual(keys(),first);
    const callsBefore=moves.length;
    const late={...c,plex:{...c.plex,albums:[...c.plex.albums,plex("late")],loading:true}};
    render({...props,context:late});assert.equal(node.scrollLeft,end);assert.deepEqual(keys(),first);assert.equal(moves.length,callsBefore,"late catalogs do not reset scroll");
    node.clientWidth=1600;resized();render();assert.deepEqual(keys(),first);assert.ok(node.scrollLeft<=node.scrollWidth-node.clientWidth);
    assert.equal(h.slots[0].value.getSnapshot().selected.length,10);
    namedButton(tree,translations.en.t("explore.renew")).props.onClick();render();assert.equal(node.scrollLeft,0);assert.equal(keys().length,10);
    node.clientWidth=2200;resized();render();assert.equal(arrow("explore.previousSuggestions").props.disabled,true);assert.equal(arrow("explore.nextSuggestions").props.disabled,true);
    const before=moves.length;arrow("explore.nextSuggestions").props.onClick();assert.equal(moves.length,before);
  } finally {h.close();Object.assign(globalThis,previous);}
});

test("horizontal reservation and short inventory retain all cards, with no synthetic padding or duplicate suggestions", () => {
  globalThis.__exploreTranslation=translations.en;const c=context(0);c.local={albums:[],state:"loading"};c.plex.state="loading";
  const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}};
  const h=host(ExploreView,props);let tree=h.render();
  const viewport=nodes(tree,n=>n.props?.id==="explore-album-shelf")[0];assert.equal(viewport.props.children.length,EXPLORE_SUGGESTION_COUNT);
  assert.equal(viewport.props["aria-busy"],true);assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,0);
  const ready={...c,local:{albums:[local("one"),local("two")],state:"ready"},plex:{...c.plex,state:"empty"}};
  tree=h.render({...props,context:ready});assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,2);
  assert.equal(nodes(tree,n=>n.props?.id==="explore-album-shelf")[0].props.children.length,2);h.close();
});


test("Explore Local covers share the existing resolver, distinguish missing/failure and retry only on activation", async () => {
  globalThis.__exploreTranslation=translations.en;
  const c=context(1),s=createExploreSession(()=>0.5);s.syncContext(c,true);
  const entry=s.visibleEntries().find(e=>e.source==="local");let localReads=0,onlineReads=0,outcome="success";
  const pending=deferred();
  const resolver=new LocalArtworkResolver(async()=>{localReads++;return null;},async()=>{
    onlineReads++;if(outcome==="error")throw Error("synthetic failure");if(outcome==="missing")return null;return pending.promise;
  });resolver.setContext(false,c.local.albums);globalThis.__exploreResolver=resolver;
  const props={entry,revision:s.getSnapshot().revision,active:true,onlineArtworkEnabled:true,current:()=>s.validDetail(entry),onOpen(){}};
  const a=host(ExploreAlbumCard,props),b=host(ExploreAlbumCard,props);
  try {
    a.render();a.effects();b.render();b.effects();await flush();assert.equal(localReads,1);assert.equal(onlineReads,1);
    assert.equal(a.slots[1],"loading");const before=s.getSnapshot();
    a.render({...props,active:false});a.effects();
    pending.resolve("data:image/png;base64,synthetic");await flush();
    assert.equal(a.slots[0],null,"hidden consumer cannot present late success");assert.equal(b.slots[0],"data:image/png;base64,synthetic","another active consumer keeps the shared query valid");
    a.render(props);a.effects();
    assert.equal(a.slots[0],"data:image/png;base64,synthetic");assert.equal(b.slots[0],a.slots[0]);assert.equal(a.slots[1],"ready");assert.equal(s.getSnapshot(),before);
    for(let i=0;i<10;i++){a.render();a.effects();}assert.equal(onlineReads,1);
  } finally {a.close();b.close();delete globalThis.__exploreResolver;}
  for(const initial of ["missing","error"]){
    let requests=0,fail=initial==="error";
    const r=new LocalArtworkResolver(async()=>null,async()=>{requests++;if(fail)throw Error("transient");return null;});r.setContext(false,c.local.albums);globalThis.__exploreResolver=r;
    const h=host(ExploreAlbumCard,props);
    try {
      h.render();h.effects();await flush();assert.equal(h.slots[1],initial);assert.equal(r.get(entry.album.id),null);
      for(let i=0;i<10;i++){h.render();h.effects();}assert.equal(requests,1);
      fail=false;h.render({...props,active:false});h.effects();h.render(props);h.effects();await flush();
      assert.equal(requests,2);assert.equal(h.slots[1],"missing");
    }finally{h.close();delete globalThis.__exploreResolver;}
  }
});

test("Explore cover preference and visibility gates prevent online I/O and obsolete consumers cannot publish", async()=>{
  globalThis.__exploreTranslation=translations.en;
  const c=context(1),s=createExploreSession(()=>0.5);s.syncContext(c,true);const entry=s.visibleEntries().find(e=>e.source==="local");
  const previous=globalThis.IntersectionObserver;let intersect,localReads=0,onlineReads=0;
  globalThis.IntersectionObserver=class{constructor(callback){intersect=callback;}observe(){}disconnect(){}};
  const r=new LocalArtworkResolver(async()=>{localReads++;return null;},async()=>{onlineReads++;return "online";});r.setContext(false,c.local.albums);globalThis.__exploreResolver=r;
  const props={entry,revision:s.getSnapshot().revision,active:true,onlineArtworkEnabled:false,current:()=>s.validDetail(entry),onOpen(){}};
  const h=host(ExploreAlbumCard,props);
  try {
    let tree=h.render();tree.props.ref.current={};h.effects();await flush();assert.equal(localReads,0);
    intersect([{isIntersecting:true}]);h.render();h.effects();await flush();assert.equal(localReads,1);assert.equal(onlineReads,0);assert.equal(h.slots[1],"missing");
    h.render({...props,onlineArtworkEnabled:true});h.effects();await flush();assert.equal(onlineReads,1);assert.equal(h.slots[0],"online");
    const image=nodes(h.render(),n=>n.type==="img")[0];image.props.onError();assert.equal(nodes(h.render(),n=>n.type==="img").length,0);
  }finally{h.close();globalThis.IntersectionObserver=previous;delete globalThis.__exploreResolver;}
  const pending=deferred();let allowed=true;
  const stale=new LocalArtworkResolver(async()=>null,()=>pending.promise);stale.setContext(false,c.local.albums);globalThis.__exploreResolver=stale;
  const old=host(ExploreAlbumCard,{...props,onlineArtworkEnabled:true,current:()=>allowed});
  try{old.render();old.effects();await flush();allowed=false;pending.resolve("obsolete");await flush();assert.equal(old.slots[0],null);assert.equal(stale.get(entry.album.id),null);}
  finally{old.close();delete globalThis.__exploreResolver;}
});

test("a cover published by another existing consumer reaches a mounted Explore card without resampling",async()=>{
  globalThis.__exploreTranslation=translations.en;const c=context(1),s=createExploreSession(()=>0.5);s.syncContext(c,true);
  const entry=s.visibleEntries().find(e=>e.source==="local"),r=new LocalArtworkResolver(async()=>null,async()=>"shared");
  globalThis.__exploreResolver=r;const h=host(ExploreAlbumCard,{entry,revision:s.getSnapshot().revision,active:true,artworkBlocked:true,current:()=>s.validDetail(entry),onOpen(){}});
  try{h.render();h.effects();const before=s.getSnapshot();await r.resolve(entry.album,true,false);assert.equal(h.slots[0],"shared");assert.equal(s.getSnapshot(),before);}
  finally{h.close();delete globalThis.__exploreResolver;}
});


test("initial configuration waits before onboarding, and source configuration is independent of catalog contents or previous setup",()=>{
  for(const language of ["pt-BR","en"]){
    globalThis.__exploreTranslation=translations[language];const t=translations[language].t.bind(translations[language]);
    const c=context(0);c.local.state="not_configured";c.plex.state="not_configured";
    let configured=0,reads=0;const props={context:c,active:true,configurationReady:false,onConfigure(){configured++;},ensureLocal(){reads++;},isCurrent:()=>true,onOpen(){}};
    const h=host(ExploreView,props);let tree=h.render();h.effects();
    assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(t("explore.creatingSuggestions")));assert.equal(namedButton(tree,t("explore.configureCollection")),undefined);assert.equal(reads,0);
    tree=h.render({...props,configurationReady:true});h.effects();
    assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(t("explore.collectionStartTitle")));assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,0);
    assert.equal(namedButton(tree,t("explore.renew")),undefined);assert.equal(nodes(tree,n=>n.props?.["aria-controls"]==="explore-album-shelf").length,0);
    namedButton(tree,t("explore.configureCollection")).props.onClick();assert.equal(configured,1);assert.equal(reads,0);
    const ready={...c,local:{albums:[local("first")],state:"ready"}};
    tree=h.render({...props,configurationReady:true,context:ready});h.effects();assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,1);
    assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(t("explore.localScope")));assert.equal(h.slots[0].value.getSnapshot().preparing,false);h.close();
  }
  // The actual App bootstrap blocks all screens until both persisted config and section have loaded.
  let bootstrap;const visit=n=>{if(ts.isIfStatement(n)&&n.expression.getText(appAst)==="!config || mediaSource === null")bootstrap=n.expression.getText(appAst);ts.forEachChild(n,visit);};visit(appAst);assert.ok(bootstrap);
  const check=new Function("config","mediaSource",`return (${bootstrap});`);
  assert.equal(check(null,"explore"),true);assert.equal(check({first_run:false,local_folders:[],plex_token:""},"explore"),false);
});

test("Local-only, Plex-only and a healthy source alongside failure publish without waiting for unconfigured sources",()=>{
  for(const sourceName of ["local","plex"])for(const otherState of ["not_configured","unavailable","loading"]){
    const c=context(5),other=sourceName==="local"?"plex":"local";c[other]={...c[other],albums:[],state:otherState};
    const s=createExploreSession(()=>0.5);s.syncContext(c,true);assert.equal(s.getSnapshot().preparing,false);assert.equal(s.visibleEntries().length,5);
    assert.ok(s.visibleEntries().every(e=>e.source===sourceName));const before=s.visibleEntries().map(e=>e.key);
    s.syncContext(c,false);s.syncContext(c,true);assert.deepEqual(s.visibleEntries().map(e=>e.key),before);
    globalThis.__exploreTranslation=translations.en;const h=host(ExploreView,{context:c,active:true,ensureLocal(){},isCurrent:()=>true,onOpen(){},onConfigure(){}});
    const tree=h.render();assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,5);
    assert.ok(!renderToStaticMarkup(shallowMarkup(tree)).includes(translations.en.t("explore.collectionStartTitle")));
    assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(translations.en.t(otherState==="not_configured"?`explore.${sourceName}Scope`:"explore.aggregateScope")));h.close();
  }
});

test("configured empty or failed sources retain configuration actions and useful retries, with loading distinct from empty",()=>{
  globalThis.__exploreTranslation=translations.en;const t=translations.en.t.bind(translations.en);
  for(const phase of ["empty","unavailable","loading"]){
    const c=context(0);c.local.state=phase;c.plex.state="not_configured";let retry=0,configure=0;
    const props={context:c,active:true,ensureLocal(){},isCurrent:()=>true,onOpen(){},onConfigure(){configure++;},retryLocal(){retry++;}};
    const h=host(ExploreView,props);let tree=h.render();const markup=renderToStaticMarkup(shallowMarkup(tree));
    assert.ok(!markup.includes(t("explore.collectionStartTitle")));
    if(phase==="loading"){assert.ok(markup.includes(t("explore.creatingSuggestions")));assert.ok(!markup.includes(t("explore.collectionEmpty")));}
    else {assert.ok(markup.includes(t(phase==="empty"?"explore.collectionEmpty":"explore.collectionUnavailable")));assert.equal(nodes(tree,n=>n.props?.id==="explore-album-shelf").length,0);assert.equal(namedButton(tree,t("explore.renew")),undefined);namedButton(tree,t("explore.configureCollection")).props.onClick();assert.equal(configure,1);}
    if(phase==="unavailable"){namedButton(tree,t("explore.retryLocal")).props.onClick();assert.equal(retry,1);tree=h.render({...props,canRetryLocal:false});assert.equal(namedButton(tree,t("explore.retryLocal")).props.disabled,true);}
    h.close();
  }
});


test("App reopens the existing wizard as an overlay; cancel only closes it, completion updates config and accepted saves await the monitor",()=>{
  let wizardNode,exploreNode;
  const visit=n=>{if(ts.isJsxSelfClosingElement(n)){
    const tag=n.tagName.getText(appAst);if(tag==="WelcomeWizard"&&n.attributes.properties.some(p=>p.name?.getText(appAst)==="onCancel"))wizardNode=n;
    if(tag==="ExploreView")exploreNode=n;
  }ts.forEachChild(n,visit);};visit(appAst);assert.ok(wizardNode);assert.ok(exploreNode);
  const attr=(node,name)=>node.attributes.properties.find(p=>p.name?.getText(appAst)===name)?.initializer?.expression?.getText(appAst);
  const config={first_run:false,local_folders:[],plex_token:""},events=[];let opened=false,revision=4;
  const scope={config,setShowCollectionWizard:v=>{opened=v;events.push(["open",v]);},setConfig:v=>events.push(["config",v]),setLocalCatalogRevision:fn=>{revision=fn(revision);},configSaveMonitorRef:{current:{accepted:op=>events.push(["accepted",op])}}};
  const run=(node,name)=>new Function(...Object.keys(scope),`${compile("const extracted = " + attr(node,name))}; return extracted;`)(...Object.values(scope));
  assert.equal(new Function("config",`return (${attr(wizardNode,"initialConfig")});`)(config),config);
  run(exploreNode,"onConfigure")();assert.equal(opened,true);run(wizardNode,"onCancel")();assert.equal(opened,false);assert.deepEqual(events,[["open",true],["open",false]]);
  run(exploreNode,"onConfigure")();const next={...config,local_folders:["/fixture/first"]};run(wizardNode,"onFinish")(next);assert.equal(opened,false);assert.equal(revision,5);assert.ok(events.some(e=>e[0]==="config"&&e[1]===next));
  const count=events.filter(e=>e[0]==="config").length,operation={id:"fixture-operation",phase:"preparing"};run(wizardNode,"onSaveAccepted")(operation);
  assert.equal(events.filter(e=>e[0]==="config").length,count);assert.ok(events.some(e=>e[0]==="accepted"&&e[1]===operation));assert.equal(config.local_folders.length,0);
});

test("three native shelves retain independent positions, reset only the renewed shelf and survive artist detail return",()=>{
  globalThis.__exploreTranslation=translations.en;const previous={window:globalThis.window,ResizeObserver:globalThis.ResizeObserver};
  globalThis.window={matchMedia:()=>({matches:true})};globalThis.ResizeObserver=class{constructor(fn){this.fn=fn;}observe(node){node.resize=this.fn;}disconnect(){}};
  const c=context(0);c.local.albums=Array.from({length:30},(_,i)=>({...local(`theme-${i}`),title:`Album ${i}`,year:"1995",genres:["Jazz"]}));
  const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}};
  const h=host(ExploreView,props),tracks=new Map();let tree,current=props;
  const render=(next=current)=>{current=next;for(let i=0;i<3;i++){
    tree=h.render(current);
    for(const viewport of nodes(tree,n=>n.props?.id?.startsWith("explore-")&&n.props.role==="region")){
      const id=viewport.props.id;let node=tracks.get(id);
      if(!node){const listeners=new Map();node={clientWidth:600,scrollLeft:0,cardWidth:190,count:10,moves:[],
        get scrollWidth(){return this.count*this.cardWidth+(this.count-1)*24;},
        addEventListener(event,fn){listeners.set(event,fn);},removeEventListener(event,fn){if(listeners.get(event)===fn)listeners.delete(event);},
        scrollTo(options){this.moves.push(options);this.scrollLeft=Math.max(0,Math.min(options.left,this.scrollWidth-this.clientWidth));listeners.get("scroll")?.();}};tracks.set(id,node);}
      viewport.props.ref.current=node;node.cardWidth=parseFloat(viewport.props.style.gridAutoColumns);node.count=viewport.props.children.length;
    }h.effects();
  }return tree;};
  const arrow=id=>nodes(tree,n=>n.type==="button"&&n.props["aria-controls"]===id&&n.props["aria-label"]===translations.en.t("explore.nextSuggestions"))[0];
  const lefts=()=>[...tracks].map(([id,node])=>[id,node.scrollLeft]);
  try{
    render();assert.equal(tracks.size,3);const initial=h.slots[0].value.getSnapshot();
    for(const id of tracks.keys()){arrow(id).props.onClick();render();assert.ok(tracks.get(id).scrollLeft>0);}
    const positions=lefts(),moves=[...tracks.values()].map(n=>n.moves.length);
    render();assert.deepEqual(lefts(),positions);assert.deepEqual([...tracks.values()].map(n=>n.moves.length),moves);assert.equal(h.slots[0].value.getSnapshot(),initial);
    const refresh=nodes(tree,n=>n.type==="button"&&n.props["aria-label"]===translations.en.t("explore.renewSection",{title:translations.en.t("explore.genreTheme",{genre:"Jazz"})}))[0];
    refresh.props.onClick();render();assert.equal(tracks.get("explore-genre-shelf").scrollLeft,0);
    assert.equal(tracks.get("explore-decade-shelf").scrollLeft,positions.find(([id])=>id==="explore-decade-shelf")[1]);assert.equal(tracks.get("explore-album-shelf").scrollLeft,positions.find(([id])=>id==="explore-album-shelf")[1]);
    const kept=lefts();h.render({...current,active:false});h.effects();render();assert.deepEqual(lefts(),kept);
    const s=h.slots[0].value;s.openSlice(s.getSnapshot().artists[0],s.getSnapshot().revision);render();assert.equal(nodes(tree,n=>n.props.role==="region").length,0);
    s.leaveSlice(s.getSnapshot().revision);render();assert.deepEqual(lefts(),kept);
    namedButton(tree,translations.en.t("explore.renew")).props.onClick();render();assert.ok([...tracks.values()].every(n=>n.scrollLeft===0));
  }finally{h.close();Object.assign(globalThis,previous);}
});

test("artist imagery uses one associated album, retains initials on absence and replaces only the image when portrait arrives",async()=>{
  globalThis.__exploreTranslation=translations.en;
  const c=context(1);c.plex.albums[0].artist_rating_key="exact-artist";const s=createExploreSession(()=>0.5);s.syncContext(c,true);
  const group=s.getSnapshot().artists.find(g=>g.source==="local"),entry=s.artistAlbum(group);
  let reads=0;const r=new LocalArtworkResolver(async()=>{reads++;return null;},async()=>assert.fail("online disabled"));r.setContext(false,c.local.albums);globalThis.__exploreResolver=r;
  const props={group,identity:c.plex.identity,serverId:"server",active:true,album:entry,currentAlbum:()=>s.validDetail(entry),onlineArtworkEnabled:false};
  const portrait=host(ExploreArtistPortrait,props);let card;
  try{
    const fallback=nodes(portrait.render(),n=>n.type===ExploreAlbumCard)[0];assert.equal(fallback.props.entry,entry);assert.equal(fallback.props.artworkOnly,true);
    card=host(ExploreAlbumCard,fallback.props);let tree=card.render();card.effects();await flush();tree=card.render();
    assert.equal(reads,1);assert.equal(nodes(tree,n=>n.type==="button").length,0);assert.equal(nodes(tree,n=>n.type==="img").length,0);assert.ok(nodes(tree,n=>n.type==="span").length);
    for(let i=0;i<5;i++){card.render();card.effects();}assert.equal(reads,1);
  }finally{portrait.close();card?.close();delete globalThis.__exploreResolver;}
  const plexGroup=s.getSnapshot().artists.find(g=>g.source==="plex");
  const pending=deferred(),cache=new PlexArtistPortraitCache(()=>pending.promise);cache.setContext(c.plex.identity,"server");const associated=s.artistAlbum(plexGroup);
  const photo=host(ExploreArtistPortrait,{group:plexGroup,identity:c.plex.identity,serverId:"server",active:true,cache,album:associated,currentAlbum:()=>s.validDetail(associated)});
  try{
    const before=s.getSnapshot();let tree=photo.render();photo.effects();await flush();assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard)[0].props.entry,associated);
    pending.resolve({server_id:"server",path:"/library/metadata/exact-artist/thumb/1"});await flush();tree=photo.render();
    assert.equal(nodes(tree,n=>n.props?.image)[0].props.image.path,"/library/metadata/exact-artist/thumb/1");assert.equal(s.getSnapshot(),before);assert.equal(s.artistAlbum(plexGroup),associated);
  }finally{photo.close();cache.invalidate();}
});


test("colored navigation genre cards show the count/source of the actual slice, including availability changes",()=>{
  for(const language of ["pt-BR","en"]){
    globalThis.__exploreTranslation=translations[language];const t=translations[language].t.bind(translations[language]);
    const c=context(1);c.local.albums[0].genres=["Jazz"," jazz ","Unknown"];c.plex.albums[0].genres=["Jazz","Desconhecido"];c.plex.albums.push(...Array.from({length:4},(_,i)=>({...plex(`extra-${i}`),genres:["Jazz"]})));
    const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}};
    const h=host(ExploreView,props);let tree=h.render();const button=namedButton(tree,"Jazz");assert.ok(button);
    assert.equal(button.props["aria-label"],`Jazz · ${t("explore.sliceCount",{count:6})} · ${t("explore.sources.local")} / ${t("explore.sources.plex")}`);
    assert.equal(namedButton(tree,"Unknown"),undefined);assert.equal(namedButton(tree,"Desconhecido"),undefined);
    button.props.onClick();tree=h.render();assert.equal(h.slots[0].value.getSnapshot().slice.entries.length,6);assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,6);
    const s=h.slots[0].value;s.leaveSlice(s.getSnapshot().revision);
    tree=h.render({...props,context:{...c,local:{...c.local,state:"unavailable"}}});const current=namedButton(tree,"Jazz");
    assert.equal(current.props["aria-label"],`Jazz · ${t("explore.sliceCount",{count:5})} · ${t("explore.sources.plex")}`);
    current.props.onClick();h.render();assert.equal(s.getSnapshot().slice.entries.length,5);assert.equal(s.getSnapshot().slice.entries[0].source,"plex");h.close();
  }
});


test("displayed genre collisions stay distinct through renders, detail return and later source arrivals",()=>{
  globalThis.__exploreTranslation=translations.en;
  const names=["Jazz","Rock","Blues","Pop","Samba","Reggae","Country","Soul","Funk","Ambient","Classical","Electronic"];
  const pair=names.flatMap((a,i)=>names.slice(i+1).filter(b=>exploreGenreCardStyle(a).background===exploreGenreCardStyle(b).background).map(b=>[a,b]))[0];assert.ok(pair);
  const selected=[...pair,"Hip Hop","R&B"],c=context(0);c.plex.state="not_configured";
  c.local.albums=selected.flatMap((genre,i)=>Array.from({length:5},(_,j)=>({...local(`${i}-${j}`),genres:[genre]})));
  const props={context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}},h=host(ExploreView,props);
  const cards=tree=>nodes(tree,n=>n.type==="button"&&n.props.style?.backgroundColor);
  const colors=tree=>Object.fromEntries(cards(tree).map(n=>[nodes(n,p=>p.type==="p"&&p.props.title===p.props.children)[0].props.children,n.props.style.backgroundColor]));
  const icons=tree=>Object.fromEntries(cards(tree).map(n=>[nodes(n,p=>p.type==="p"&&p.props.title===p.props.children)[0].props.children,nodes(n,p=>p.props["aria-hidden"]==="true")[0].type]));
  try{
    let tree=h.render();h.effects();const before=colors(tree);assert.equal(Object.keys(before).length,4);assert.equal(new Set(Object.values(before)).size,4);
    const beforeIcons=icons(tree);assert.equal(new Set(Object.values(beforeIcons)).size,4);
    for(let i=0;i<5;i++){tree=h.render();h.effects();assert.deepEqual(colors(tree),before);assert.deepEqual(icons(tree),beforeIcons);}
    h.render({...props,active:false});h.effects();tree=h.render(props);h.effects();assert.deepEqual(colors(tree),before);assert.deepEqual(icons(tree),beforeIcons);
    const copies=c.local.albums.slice(0,2).map(a=>({rating_key:a.id,title:a.title,artist:a.artist,genres:a.genres}));
    tree=h.render({...props,context:{...c,plex:{...c.plex,state:"ready",albums:copies}}});h.effects();assert.deepEqual(colors(tree),before);assert.deepEqual(icons(tree),beforeIcons);
  }finally{h.close();}
});

test("Rock variants open the same real navigation slice and name the album theme Rock in both languages",()=>{
  for(const language of ["pt-BR","en"]){
    globalThis.__exploreTranslation=translations[language];const c=context(0),variants=["Rock","Rock & Roll","Rock and Roll"];
    c.local.albums=Array.from({length:10},(_,i)=>({...local(`family-${i}`),genres:[variants[i%3],"Rock & Roll"]}));
    c.plex.albums=c.local.albums.map(a=>({...plex(a.id,a.artist),genres:["Rock and Roll"]}));
    const h=host(ExploreView,{context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}});
    try{
      let tree=h.render();const button=namedButton(tree,"Rock");assert.ok(button);
      assert.equal(namedButton(tree,"Rock & Roll"),undefined);assert.equal(namedButton(tree,"Rock and Roll"),undefined);
      assert.ok(nodes(tree,n=>n.type==="h2"&&n.props.children===translations[language].t("explore.genreTheme",{genre:"Rock"})).length);
      button.props.onClick();tree=h.render();const slice=h.slots[0].value.getSnapshot().slice;
      assert.equal(slice.entries.length,20);assert.equal(new Set(slice.entries.map(e=>e.key)).size,20);
      assert.deepEqual(slice.entries.map(e=>e.album),[...c.local.albums,...c.plex.albums]);assert.equal(nodes(tree,n=>n.type===ExploreAlbumCard).length,6);
    }finally{h.close();}
  }
});

test("the decade shelf title uses four digits in both languages",()=>{
  for(const language of ["pt-BR","en"]){
    globalThis.__exploreTranslation=translations[language];const c=context(0);c.plex.state="not_configured";
    c.local.albums=Array.from({length:12},(_,i)=>({...local(`decade-${i}`),year:"2015"}));
    const h=host(ExploreView,{context:c,active:true,libraryTitle:null,ensureLocal(){},isCurrent:()=>true,onOpen(){}});
    const tree=h.render(),title=translations[language].t("explore.pastDecade",{decade:2010});
    assert.ok(nodes(tree,n=>n.type==="h2"&&n.props.children===title).length);assert.equal(nodes(tree,n=>n.type==="h2"&&n.props.children===translations[language].t("explore.pastDecade",{decade:"10"})).length,0);h.close();
  }
});
