import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { createExploreSession } from "./explore.ts";
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

// The actual session uses injected randomness only in these synthetic tests.
test("six suggestions retain exact homonymous editions, artist diversity and both sources without excluding incomplete metadata", () => {
  const c = context(), session = createExploreSession(() => 0.4); session.syncContext(c, true);
  const selected = session.getSnapshot().selected;
  assert.equal(selected.length, 6); assert.equal(new Set(selected.map(e => e.album.artist)).size, 6);
  assert.equal(new Set(selected.map(e => e.source)).size, 2); assert.equal(new Set(selected.map(e => e.key)).size, 6);
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

test("pending sources reserve slots; late inventory fills without reshuffling prior identities and errors leave the other source usable", () => {
  const c = context(), s = createExploreSession(() => 0.5);
  s.syncContext({ ...c, local: { albums: [], state: "loading" } }, true); const partial = s.getSnapshot().selected;
  assert.equal(partial.length, 3); s.syncContext(c, true);
  assert.equal(s.getSnapshot().selected.length, 6); assert.deepEqual(s.getSnapshot().selected.slice(0, 3), partial);
  const unavailable = createExploreSession(() => 0.5); unavailable.syncContext({ ...c, local: { ...c.local, state: "unavailable" } }, true);
  assert.equal(unavailable.visibleEntries().length, 6); unavailable.syncContext(c, true);
  assert.equal(new Set(unavailable.visibleEntries().map(e => e.source)).size, 2);
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
 ${["useState", "useRef", "useMemo", "useEffect", "useSyncExternalStore"].map(name => `export const ${name} = (...args) => globalThis.__exploreHost.${name}(...args);`).join("\n")}`);
const translationUrl = dataModule('export const useTranslation = () => ({t: (...args) => globalThis.__exploreTranslation.t(...args)});');
const artworkUrl = dataModule('export const localArtworkResolver = { get: () => null, resolveLocalOnly: (...args) => globalThis.__exploreCover(...args) };');
const plexImageUrl = dataModule('export const PlexImage = () => null;');
const placeholderUrl = dataModule('export const AlbumArtworkPlaceholder = () => null;');
const componentUrl = dataModule(compile(source("../components/ExploreView.tsx"))
  .replace('from "react"', `from ${JSON.stringify(hooksUrl)}`)
  .replace(/from "(react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/explore"', `from ${JSON.stringify(new URL("./explore.ts", import.meta.url).href)}`)
  .replace('from "../utils/localArtworkSession"', `from ${JSON.stringify(artworkUrl)}`)
  .replace('from "./PlexImage"', `from ${JSON.stringify(plexImageUrl)}`)
  .replace('from "./AlbumArtworkPlaceholder"', `from ${JSON.stringify(placeholderUrl)}`));
const { ExploreView, ExploreAlbumCard } = await import(componentUrl);
function host(Component, props) {
  let cursor = 0; const slots = [], effects = [];
  const h = {
    useState(initial) { const i = cursor++; if (!(i in slots)) slots[i] = typeof initial === "function" ? initial() : initial; return [slots[i], v => slots[i] = typeof v === "function" ? v(slots[i]) : v]; },
    useRef(initial) { return slots[cursor++] ??= { current: initial }; },
    useMemo(fn, deps) { const i = cursor++; if (!slots[i] || deps.some((v,j) => v !== slots[i].deps[j])) slots[i] = {deps,value:fn()}; return slots[i].value; },
    useEffect(fn,deps) { const i = cursor++, old = slots[i]; if (!old || deps.some((v,j)=>v!==old.deps[j])) effects.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()};}); },
    useSyncExternalStore(_,get) { return get(); },
    render(next=props) { props=next;cursor=0;globalThis.__exploreHost=h;return Component(props); },
    effects() { effects.splice(0).forEach(fn=>fn()); }, close() {slots.forEach(s=>s?.cleanup?.());},slots,
  }; return h;
}
function nodes(tree,predicate) { if(Array.isArray(tree))return tree.flatMap(n=>nodes(n,predicate));if(!tree||typeof tree!=="object")return[];return[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)]; }
function shallowMarkup(tree) {
  if(Array.isArray(tree))return tree.map((child,index)=>{
    const node=shallowMarkup(child);
    return React.isValidElement(node)&&node.key===null?React.cloneElement(node,{key:index}):node;
  });
  if(!React.isValidElement(tree))return tree;
  if(tree.type===ExploreAlbumCard)return React.createElement("article",{key:tree.key},tree.props.entry.album.title);
  return React.cloneElement(tree,{},shallowMarkup(tree.props.children));
}

test("real Explore shelf shows independent source states and selected library, without exposing errors or credentials", () => {
  for(const language of ["pt-BR","en"])for(const phase of ["loading","empty","not_configured","unavailable","ready"]){
    globalThis.__exploreTranslation=translations[language];const c=context();c.local.state=phase;
    const h=host(ExploreView,{context:c,active:true,libraryTitle:"Selected library",ensureLocal(){},isCurrent:()=>true,onOpen(){}});
    const tree=h.render(),markup=renderToStaticMarkup(shallowMarkup(tree));
    assert.ok(markup.includes(translations[language].t(`explore.states.${phase}`)));assert.ok(markup.includes("Selected library"));
    assert.ok(nodes(tree,n=>n.type===ExploreAlbumCard).some(n=>n.props.entry.source==="plex"));h.close();
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
  const localState=new Function("localExploreConfigured","isPlaybackAvailable","playbackStatus","localCatalog",`return (${expression("localExploreState")});`);
  const base={loaded:true,loading:false,saveBlocked:false,mpdBusy:false,error:null,albums:[local("one")]};
  assert.equal(localState(false,true,{is_updating:false},base),"not_configured");assert.equal(localState(true,false,{is_updating:false},base),"unavailable");
  assert.equal(localState(true,true,{is_updating:true},base),"unavailable");assert.equal(localState(true,true,{is_updating:false},{...base,loaded:false}),"loading");
  assert.equal(localState(true,true,{is_updating:false},{...base,albums:[]}),"empty");assert.equal(localState(true,true,{is_updating:false},{...base,error:"secret error"}),"unavailable");
  const plexState=new Function("isPlexConnected","plexLibrariesIdentity","plexConnectionIdentity","loadingLibraries","plexLibrariesError","selectedLibrary","plexAlbumLoad","explorePlexIdentity","albums",`return (${expression("plexExploreState")});`);
  const conn={},scope={},args=[true,conn,conn,false,false,{key:"library"},{identity:scope,phase:"ready"},scope,[plex("one")]];
  assert.equal(plexState(...args),"ready");for(const [index,value,expected]of[[0,false,"not_configured"],[1,{},"loading"],[4,true,"unavailable"],[5,null,"not_configured"],[6,{identity:{},phase:"ready"},"loading"],[6,{identity:scope,phase:"error"},"unavailable"],[8,[],"empty"]]){const changed=[...args];changed[index]=value;assert.equal(plexState(...changed),expected);}
});

test("existing Plex album query publishes error or data once and drops disposed/generation-obsolete responses without another query", async () => {
  for(const scenario of ["success","error","disposed","generation"]){
    const calls=[],pending=deferred(),status=[],published=[],identity={},generation={current:0},conn={};
    const setters=["setAlbums","setCollections","setHasCollections","setLoading","setActiveAlbum","setActiveCollection","setActiveArtist","setFallbackPlexArtist","setSelectedPlexDecade","setSelectedPlexYear","setSelectedPlexUnknownYear","setSelectedPlexGenre","setSearchQuery","setSearchResults","setPlexViewMode"];
    const values=setters.map(name=>name==="setAlbums"?v=>published.push(v):()=>{});
    const run=new Function("selectedLibrary","plexLibrariesIdentity","plexConnectionIdentity","explorePlexIdentity","plexLibraryRequestGenerationRef","setPlexAlbumLoad","plexService","logError",...setters,`return (${effect("plexService.getAlbums(selectedLibrary.key")});`)(
      {key:"exact-library"},conn,conn,identity,generation,v=>status.push(v),{getAlbums:(...a)=>{calls.push(a);return pending.promise;},getCollections:()=>Promise.resolve([])},()=>{},...values);
    const cleanup=run();assert.deepEqual(calls,[["exact-library","added"]]);assert.equal(status[0].phase,"loading");
    if(scenario==="disposed")cleanup();if(scenario==="generation")generation.current++;
    if(scenario==="error")pending.reject(Error("DO_NOT_EXPOSE"));else pending.resolve([plex("exact")]);await flush();
    assert.equal(calls.length,1);assert.equal(status.at(-1).phase,scenario==="success"?"ready":scenario==="error"?"error":"loading");
    if(scenario==="success")assert.equal(published.at(-1)[0].rating_key,"exact");else assert.deepEqual(published,[[]]);cleanup();
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
  const current=new Function("exploreLive",`return (${expression("exploreEntryCurrent")});`)(live);
  const open=new Function("exploreLive","exploreEntryCurrent","setExploreDetail","setExploreLocalSubview",`return (${expression("openExploreAlbum")});`)(live,current,e=>changes.push(e),v=>assert.equal(v,"browser"));
  open(entry);assert.equal(changes[0],entry);live.current.visible=false;open(entry);assert.equal(changes.length,1);
  live.current={context:{...c,local:{...c.local,albums:[]},plex:{...c.plex,identity:{}}},visible:true};open(entry);assert.equal(changes.length,1);
  const close=new Function("document","setExploreDetail",`return (${expression("closeExploreAlbum")});`)({querySelector:()=>null},e=>changes.push(e));close();assert.equal(changes.at(-1),null);
  assert.match(appSource,/ExploreView context=\{exploreContext\}/);assert.match(appSource,/initialAlbumId=\{exploreDetail\.album\.id\}/);
  assert.match(appSource,/album=\{exploreDetail\.album\} onBack=\{closeExploreAlbum\}/);
  assert.match(appSource,/onlineArtworkEnabled=\{false\}/);
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
