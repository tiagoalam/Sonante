import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { createExploreSession, deriveExploreGroups, EXPLORE_PAGE_SIZE, EXPLORE_SUGGESTION_COUNT, exploreEntryIsCurrent } from "./explore.ts";
import { exploreGenreCardStyle } from "./exploreGenreCards.ts";
import { localLibrarySourceOptions, validLocalLibrarySourceSelection, filterLocalAlbumsByScope } from "./localLibraryScope.ts";
import { createLocalAlbumPlayback } from "./localAlbumPlayback.ts";
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

test("selection is stable under rerenders/detail return; renewal is in memory and changes selection when alternatives exist", () => {
  const c = context(), s = createExploreSession(() => 0.5); s.syncContext(c, true); const before = s.getSnapshot();
  for (let i = 0; i < 20; i++) s.syncContext({ local: { ...c.local }, plex: { ...c.plex } }, true);
  assert.equal(s.getSnapshot(), before);
  s.syncContext(c, false); assert.deepEqual(s.getSnapshot().selected.map(e => e.key), before.selected.map(e => e.key));
  s.syncContext(c, true); assert.deepEqual(s.getSnapshot().selected.map(e => e.key), before.selected.map(e => e.key));
  s.renew(); assert.notDeepEqual(s.getSnapshot().selected.map(e => e.key), before.selected.map(e => e.key));
  assert.equal(c.local.albums.length, 8); assert.equal(c.plex.albums.length, 8);
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
const genreSliceUrl = dataModule('export const ExploreGenreSlice = () => null;');
const placeholderUrl = dataModule('export const AlbumArtworkPlaceholder = () => null;');
const componentUrl = dataModule(compile(source("../components/ExploreView.tsx"))
  .replace('from "react"', `from ${JSON.stringify(hooksUrl)}`)
  .replace(/from "(react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/frontendDiagnostics"', `from ${JSON.stringify(new URL("./frontendDiagnostics.ts", import.meta.url).href)}`)
  .replace('from "../utils/artistInitials"', `from ${JSON.stringify(new URL("./artistInitials.ts", import.meta.url).href)}`)
  .replace('from "../utils/exploreGenreCards"', `from ${JSON.stringify(new URL("./exploreGenreCards.ts", import.meta.url).href)}`)
  .replace('from "../utils/exploreAlbumShelf"', `from ${JSON.stringify(new URL("./exploreAlbumShelf.ts", import.meta.url).href)}`)
  .replace('from "../utils/explore"', `from ${JSON.stringify(new URL("./explore.ts", import.meta.url).href)}`)
  .replace('from "../utils/localArtworkSession"', `from ${JSON.stringify(artworkUrl)}`)
  .replace('from "./PlexImage"', `from ${JSON.stringify(plexImageUrl)}`)
  .replace('from "./ExploreGenreSlice"', `from ${JSON.stringify(genreSliceUrl)}`)
  .replace('from "./AlbumArtworkPlaceholder"', `from ${JSON.stringify(placeholderUrl)}`));
const { ExploreGenreSlice } = await import(genreSliceUrl);
const { ExploreView, ExploreAlbumCard, ExploreArtistArtwork: ExploreArtistPortrait, ExploreAlbumShelf } = await import(componentUrl);
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
  if(tree.type===ExploreGenreSlice)return React.createElement("div",{key:tree.key});
  if(tree.type===ExploreArtistPortrait)return React.createElement("div",{key:tree.key});
  if(tree.type===ExploreAlbumCard)return React.createElement("article",{key:tree.key},tree.props.entry.album.title);
  return React.cloneElement(tree,{},shallowMarkup(tree.props.children));
}

test("actual shelf card opens its exact entry, returns to the same selection, renews without queries and rejects stale handlers", () => {
  globalThis.__exploreTranslation=translations.en;const c=context(),opened=[];
  const props={context:c,active:true,libraryTitle:"Selected",ensureLocal(){},isCurrent:()=>true,onOpen:e=>opened.push(e)};
  const h=host(ExploreView,props);let tree=h.render();const before=nodes(tree,n=>n.type===ExploreAlbumCard),entry=before[0];
  const card=host(ExploreAlbumCard,entry.props);const button=nodes(card.render(),n=>n.type==="button")[0];button.props.onClick();assert.equal(opened[0],entry.props.entry);
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

test("existing Plex album query publishes error or data once and drops disposed/generation-obsolete responses without another query", async () => {
  for(const scenario of ["success","error","disposed","generation"]){
    const calls=[],pending=deferred(),status=[],published=[],identity={},generation={current:0},conn={};
    const setters=["setAlbums","setCollections","setHasCollections","setLoading","setActiveAlbum","setActiveCollection","setActiveArtist","setFallbackPlexArtist","setSelectedPlexDecade","setSelectedPlexYear","setSelectedPlexUnknownYear","setSelectedPlexGenre","setSearchQuery","setSearchResults","setPlexViewMode"];
    const values=setters.map(name=>name==="setAlbums"?v=>published.push(v):()=>{});
    const run=new Function("centralSection","plexArea","selectedLibrary","plexLibrariesIdentity","plexConnectionIdentity","plexBrowserIdentity","plexLibraryRequestGenerationRef","setPlexAlbumLoad","plexService","plexCatalog","plexAlbumLoad","logError",...setters,`return (${effect("plexCatalog.loadLibrary(selectedLibrary")});`)(
      "plex","library",{key:"exact-library"},conn,conn,identity,generation,v=>status.push(v),{getCollections:()=>Promise.resolve([])},{loadLibrary:(...a)=>{calls.push(a);return pending.promise;}},null,()=>{},...values);
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

test("App's actual Local open/back handlers preserve focused card and reject stale context",()=>{
 const c=context(),session=createExploreSession(()=>0.5);session.syncContext(c,true);const entry=session.visibleEntries()[0];
 const live={current:{context:c,visible:true}},changes=[],focus=[];class Element{focus(options){focus.push(options);}}
 const card=new Element(),returnRef={current:null};const document={activeElement:card,querySelector:()=>null};
 const current=new Function("exploreLive","exploreEntryIsCurrent",`return (${expression("exploreEntryCurrent")});`)(live,exploreEntryIsCurrent);
 const open=new Function("exploreLive","exploreEntryCurrent","setExploreDetail","setExploreLocalSubview","exploreDetailReturnRef","document","HTMLElement",`return (${expression("openExploreAlbum")});`)(live,current,e=>changes.push(e),()=>{},returnRef,document,Element);
 open(entry);assert.equal(changes[0],entry);assert.equal(returnRef.current,card);
 live.current.visible=false;open(entry);assert.equal(changes.length,1);
 live.current={context:{local:{albums:[],state:"ready"}},visible:true};open(entry);assert.equal(changes.length,1);
 const close=new Function("document","setExploreDetail","requestAnimationFrame","exploreDetailReturnRef",`return (${expression("closeExploreAlbum")});`)(document,e=>changes.push(e),fn=>fn(),returnRef);
 close();assert.equal(changes.at(-1),null);assert.deepEqual(focus,[{preventScroll:true}]);
 document.querySelector=()=>({});const n=changes.length;close();assert.equal(changes.length,n);
 assert.match(appSource,/ExploreView context=\{exploreContext\}/);assert.match(appSource,/initialAlbumId=\{exploreDetail\.album\.id\}/);
 assert.match(appSource,/subview=\{exploreLocalSubview\} onSubviewChange=\{setExploreLocalSubview\}/);
 assert.doesNotMatch(appSource,/exploreDetail\?\.source === "plex"/);
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

function catalogWithSlices(){
  const c=context(0);c.local.albums=Array.from({length:15},(_,i)=>({...local(`L${i}`,"Artist"),title:i<4?`Other title ${i}`:"Same title",genres:["Rock"]}));
  c.plex.albums=Array.from({length:3},(_,i)=>({...plex(`P${i}`,"Artist"),artist_rating_key:"100",genres:["rock"]}));return c;
}
const namedButton=(tree,name)=>nodes(tree,n=>n.type==="button"&&(React.Children.toArray(n.props.children).includes(name)||nodes(n,c=>c.type==="p"&&c.props.children===name).length))[0];

test("real slice invalidates delayed cover and obsolete UI controls without reopening or routing another edition", async () => {
  globalThis.__exploreTranslation=translations.en;const c=catalogWithSlices(),opened=[],pending=deferred();let blocked;
  globalThis.__exploreCover=(album,guard)=>{blocked=guard;return pending.promise;};
  const props={context:c,active:true,libraryTitle:"Library",ensureLocal(){},isCurrent:()=>true,onOpen:e=>opened.push(e)};
  const h=host(ExploreView,props);let tree=h.render();h.effects();
  const group=nodes(tree,n=>n.type==="button"&&nodes(n,p=>p.type==="p"&&p.props.children==="Artist").length&&nodes(n,p=>p.type==="p"&&typeof p.props.children==="string"&&p.props.children.startsWith(translations.en.t("explore.sources.local")+" · ")).length)[0];
  group.props.onClick();
  tree=h.render();h.effects();const back=namedButton(tree,translations.en.t("explore.backToShelves")),next=namedButton(tree,translations.en.t("explore.nextPage"));
  const node=nodes(tree,n=>n.type===ExploreAlbumCard)[0],card=host(ExploreAlbumCard,node.props),button=nodes(card.render(),n=>n.type==="button")[0];card.effects();assert.equal(blocked(),false);
  tree=h.render({...props,context:{...c,local:{...c.local,albums:c.local.albums.map(a=>({...a}))}}});h.effects();
  assert.equal(blocked(),true);pending.resolve("stale");await flush();assert.equal(card.slots[0],null);
  button.props.onClick();group.props.onClick();next.props.onClick();back.props.onClick();assert.equal(opened.length,0);
  tree=h.render();assert.ok(renderToStaticMarkup(shallowMarkup(tree)).includes(translations.en.t("explore.sliceInvalidated")));
  assert.equal(namedButton(tree,translations.en.t("explore.backToShelves")),undefined);h.close();card.close();
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


test("App Configure collection opens Local preferences for absent, empty and unavailable sources without writes",()=>{
  let exploreNode,settingsNode;
  const visit=n=>{if(ts.isJsxSelfClosingElement(n)){
    if(n.tagName.getText(appAst)==="ExploreView")exploreNode=n;
    if(n.tagName.getText(appAst)==="SettingsModal")settingsNode=n;
  }ts.forEachChild(n,visit);};visit(appAst);assert.ok(exploreNode);assert.ok(settingsNode);
  const attr=(node,name)=>node.attributes.properties.find(p=>p.name?.getText(appAst)===name)?.initializer?.expression?.getText(appAst);
  const config={first_run:false,local_folders:[],plex_token:""},events=[];
  let opened=false,page="general",draft=null;
  const busy={current:false};
  const scope={config,configSaveInProgressRef:busy,
    setShowSettings:v=>{opened=v;events.push(["open",v]);},
    setSettingsInitialPage:v=>{page=v;events.push(["page",v]);},
    setLocalFoldersDraft:v=>{draft=v;events.push(["draft",v]);}};
  const run=(node,name)=>new Function(...Object.keys(scope),`${compile("const extracted = " + attr(node,name))}; return extracted;`)(...Object.values(scope));
  const configure=run(exploreNode,"onConfigure"),cancel=run(settingsNode,"onClose");
  globalThis.__exploreTranslation=translations.en;const t=translations.en.t.bind(translations.en);
  for(const phase of ["not_configured","empty","unavailable"]){
    events.length=0;
    const c=context(0);c.local.state=phase;c.plex.state="not_configured";
    const h=host(ExploreView,{context:c,active:true,configurationReady:true,onConfigure:configure,ensureLocal(){},isCurrent:()=>true,onOpen(){}});
    try {
      const tree=h.render(),button=namedButton(tree,t("explore.configureCollection"));assert.ok(button);assert.equal(button.props.disabled,false);
      button.props.onClick();assert.equal(opened,true);assert.equal(page,"local");
      assert.deepEqual(events,[["page","local"],["open",true]],"only navigation; no wizard, save, directory picker or indexing calls");
      cancel();assert.equal(opened,false);assert.equal(draft,null);
      assert.deepEqual(events.slice(2),[["open",false],["draft",null]],"Cancel only closes and drops the draft");
      assert.deepEqual(config,{first_run:false,local_folders:[],plex_token:""});
      events.length=0;busy.current=true;button.props.onClick();assert.deepEqual(events,[]);assert.equal(opened,false);busy.current=false;
      assert.equal(namedButton(h.render({context:c,active:false,onConfigure:configure,ensureLocal(){},isCurrent:()=>true,onOpen(){}}),t("explore.configureCollection")).props.disabled,true);
    } finally {h.close();}
  }
  assert.match(source("../App.tsx"),/initialPage=\{settingsInitialPage\}/);
  assert.doesNotMatch(source("../App.tsx"),/showCollectionWizard|setShowCollectionWizard|collectionWizardWasOpen/);
});

test("first_run alone retains the onboarding wizard and its completion route",()=>{
  let guard;const wizards=[];
  const visit=n=>{
    if(ts.isIfStatement(n)&&n.expression.getText(appAst)==="config && config.first_run")guard=n;
    if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(appAst)==="WelcomeWizard")wizards.push(n);
    ts.forEachChild(n,visit);
  };visit(appAst);assert.ok(guard);assert.equal(wizards.length,1);
  assert.ok(wizards[0].pos>guard.pos&&wizards[0].end<guard.end,"wizard belongs only to first_run return");
  const check=new Function("config",`return (${guard.expression.getText(appAst)});`);
  assert.equal(check({first_run:false,local_folders:[]}),false);
  assert.equal(check({first_run:true,local_folders:[]}),true);
  assert.equal(check({first_run:true,local_folders:["/fixture"]}),true);
  assert.ok(!wizards[0].attributes.properties.some(p=>["onCancel","onSaveAccepted"].includes(p.name?.getText(appAst))),"first setup cannot be cancelled into the configured app");
  const finish=wizards[0].attributes.properties.find(p=>p.name?.getText(appAst)==="onFinish").initializer.expression.getText(appAst);
  const events=[];const next={first_run:false,local_folders:["/fixture"]};
  const handler=new Function("setConfig","mainSectionPreference",`${compile("const extracted = " + finish)}; return extracted;`)(v=>events.push(["config",v]),{finishSetup:v=>events.push(["setup",v])});
  handler(next);assert.deepEqual(events,[["config",next],["setup",next]]);
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

test("Local artist imagery uses one associated album and retains initials on absence",async()=>{
  globalThis.__exploreTranslation=translations.en;
  const c=context(1);c.plex.albums[0].artist_rating_key="exact-artist";const s=createExploreSession(()=>0.5);s.syncContext(c,true);
  const group=s.getSnapshot().artists.find(g=>g.source==="local"),entry=s.artistAlbum(group);
  for(const resolvedCover of [null,"data:image/png;base64,album-cover-fixture"]){
    let reads=0;const r=new LocalArtworkResolver(async()=>{reads++;return resolvedCover;},async()=>assert.fail("online disabled"));r.setContext(false,c.local.albums);globalThis.__exploreResolver=r;
    const props={group,identity:c.plex.identity,serverId:"server",active:true,album:entry,currentAlbum:()=>s.validDetail(entry),onlineArtworkEnabled:false};
    const portrait=host(ExploreArtistPortrait,props);let card;
    try{
      const fallback=nodes(portrait.render(),n=>n.type===ExploreAlbumCard)[0];assert.equal(fallback.props.entry,entry);assert.equal(fallback.props.artworkOnly,true);
      card=host(ExploreAlbumCard,fallback.props);let tree=card.render();card.effects();await flush();tree=card.render();
      assert.equal(reads,1);assert.equal(nodes(tree,n=>n.type==="button").length,0);
      if(resolvedCover){
        assert.equal(nodes(tree,n=>n.type==="img")[0].props.src,resolvedCover);
        assert.equal(nodes(tree,n=>n.props.role==="img")[0].props["aria-label"],translations.en.t("explore.artistAlbumCover",{album:entry.album.title}));
      }else{assert.equal(nodes(tree,n=>n.type==="img").length,0);assert.ok(nodes(tree,n=>n.type==="span").length);}
      for(let i=0;i<5;i++){card.render();card.effects();}assert.equal(reads,1);
    }finally{portrait.close();card?.close();delete globalThis.__exploreResolver;}
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
      assert.equal(slice.entries.length,10);assert.equal(new Set(slice.entries.map(e=>e.key)).size,10);
      assert.deepEqual(slice.entries.map(e=>e.album),c.local.albums);assert.equal(nodes(tree,n=>n.type===ExploreGenreSlice)[0].props.entries.length,10);
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

test('Local source scope uses managed source_id including NAS, keeps All complete and selection independent',()=>{
 const albums=[{...local('a'),source_id:'managed-a',source_label:'Disk',folder_path:'/same/path'},
  {...local('b'),source_id:'managed-nas',source_label:'NAS',folder_path:'/same/path'},local('legacy')];
 const options=localLibrarySourceOptions(albums);assert.deepEqual(options.map(x=>x.id),['managed-a','managed-nas']);
 const browserSelection='managed-a',exploreSelection='managed-nas';
 assert.deepEqual(filterLocalAlbumsByScope(albums,exploreSelection,'').map(a=>a.id),['b']);assert.equal(browserSelection,'managed-a');
 assert.deepEqual(filterLocalAlbumsByScope(albums,null,''),albums);assert.equal(validLocalLibrarySourceSelection('removed-source',options),null);
 assert.deepEqual(localLibrarySourceOptions([local('no-source')]),[]);
 const run=new Function('useMemo','filterLocalAlbumsByScope','localCatalog','activeExploreSourceId',`return ${expression('exploreAlbums')};`);
 assert.deepEqual(run(fn=>fn(),filterLocalAlbumsByScope,{albums},exploreSelection).map(a=>a.id),['b']);
 assert.match(appSource,/const \[exploreSourceId, setExploreSourceId\] = useState<string \| null>\(null\)/);
});

test('source changes invalidate Local result/group/page tokens and detail, while normal renders/return never resample',()=>{
 const albums=Array.from({length:30},(_,i)=>({...local(String(i),'Same artist'),title:`Album ${i}`,genres:['Jazz'],source_id:i<15?'disk':'nas'}));
 const scope=id=>({local:{albums:filterLocalAlbumsByScope(albums,id,''),state:'ready',sourceId:id}});
 let draws=0;const s=createExploreSession(()=>{draws++;return .5;});const a=scope('disk');s.syncContext(a,true);
 const first=s.getSnapshot(),entry=first.selected[0],artist=first.artists[0];s.openSlice(artist,first.revision);s.setPage(1,s.getSnapshot().revision);
 const slice=s.getSnapshot().slice,selected=s.getSnapshot().selected,n=draws;
 for(let i=0;i<20;i++)s.syncContext({local:{...a.local}},true);assert.equal(draws,n);
 s.syncContext(a,false);s.syncContext(a,true);assert.equal(s.getSnapshot().slice,slice);assert.equal(s.getSnapshot().selected,selected);assert.equal(draws,n);
 const b=scope('nas');s.syncContext(b,true);assert.equal(s.getSnapshot().slice,null);assert.ok(s.visibleEntries().every(e=>e.album.source_id==='nas'));
 assert.equal(s.canUse(entry,first.revision),false);assert.equal(s.validDetail(entry),false);assert.equal(exploreEntryIsCurrent(b,entry),false);
 const current=s.getSnapshot();s.openSlice(artist,first.revision);s.setPage(1,first.revision);assert.equal(s.getSnapshot(),current);
 s.syncContext(scope(null),true);assert.equal(s.candidateCount(),30);assert.ok(s.visibleArtists().every(g=>g.source==='local'));
});

test('Explore has no Plex context/query/portrait; preserved browser loader is inert for Explore',async()=>{
 assert.doesNotMatch(appSource,/\.aggregate\(|retryFailures\(|exploreDetail\?\.source === "plex"/);
 assert.doesNotMatch(source('../components/ExploreView.tsx'),/PlexImage|plexArtistPortraits|context\.plex|retryPlex|plexProgress/);
 assert.doesNotMatch(source('./explore.ts'),/PlexAlbum|context\.plex|next\.plex|derivePlex/);
 const calls=[];const dummy=new Proxy({}, {get:()=>()=>{calls.push('Plex query');throw Error('unexpected Plex query');}});
 const setters=['setAlbums','setCollections','setHasCollections','setLoading'];
 const run=new Function('selectedLibrary','plexLibrariesIdentity','plexConnectionIdentity','centralSection','plexArea','plexService','plexCatalog',...setters,`return (${effect('plexCatalog.loadLibrary(selectedLibrary')});`);
 const identity={};run({key:'library'},identity,identity,'explore','library',dummy,dummy,...setters.map(()=>()=>{}))();await flush();assert.deepEqual(calls,[]);
 const c={local:{albums:[local('only')],state:'ready'}};Object.defineProperty(c,'plex',{get:()=>assert.fail('Plex context read')});
 const s=createExploreSession(()=>.5);s.syncContext(c,true);assert.equal(s.candidateCount(),1);assert.equal(s.visibleEntries()[0].source,'local');
});

test('Explore sidebar is under Local, preserves section preference and leaves Discover under Plex',()=>{
 const ast=ts.createSourceFile('App.tsx',appSource,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let explore,discover;
 const visit=node=>{if(ts.isJsxSelfClosingElement(node)&&node.tagName.getText(ast)==='SidebarNavItem'){
  const label=node.attributes.properties.find(p=>p.name?.getText(ast)==='label')?.initializer?.getText(ast);
  if(label?.includes('explore.title'))explore=node;if(label?.includes('discover.menu'))discover=node;
 }ts.forEachChild(node,visit);};visit(ast);
 assert.ok(explore&&discover);assert.equal(explore.parent.openingElement.tagName.getText(ast),'div');assert.match(explore.parent.openingElement.getText(ast),/ml-6 border-l/);
 assert.match(explore.parent.parent.getText(ast),/navigateLocalSubview\("browser"\)/);assert.match(explore.getText(ast),/mainSectionPreference.navigate\("explore"\)/);
 assert.match(discover.getText(ast),/setPlexArea\("discover"\).*navigate\("plex"\)/);
 assert.match(appSource,/new PlexDiscoverSession\(plexService.getDiscover, plexService.getDiscoverChildren, plexService.getDiscoverRecent/);
});

test('Local Explore exposes loading, empty, unavailable and configuration states in both languages with its source selector',()=>{
 for(const language of ['en','pt-BR'])for(const phase of ['loading','empty','unavailable','not_configured','ready']){
  globalThis.__exploreTranslation=translations[language];const c={local:{albums:phase==='ready'?[local('one')]:[],state:phase}};
  const selector=React.createElement('button',{'aria-label':translations[language].t('explore.selectSource')},translations[language].t('explore.allLibraries'));
  const h=host(ExploreView,{context:c,active:true,ensureLocal(){},isCurrent:()=>true,onOpen(){},sourceSelector:selector,onConfigure(){},retryLocal(){}});
  const html=renderToStaticMarkup(shallowMarkup(h.render()));
  assert.doesNotMatch(html,/explore\.|Plex/);assert.match(html,new RegExp(translations[language].t(phase==='not_configured'?'explore.collectionStartText':'explore.localScope')));
  if(phase!=='not_configured')assert.ok(html.includes(translations[language].t('explore.allLibraries')));
  h.close();
 }
});


test('actual Local Explore main scroll returns after detail and resets only on source change',()=>{
 globalThis.__exploreTranslation=translations.en;
 const albums=Array.from({length:12},(_,i)=>({...local(String(i)),source_id:'disk'}));
 const context={local:{albums,state:'ready',sourceId:null}};
 const props={context,active:true,ensureLocal(){},isCurrent:()=>true,onOpen(){}};
 const h=host(ExploreView,props),node={scrollTop:0};
 const scroll=tree=>nodes(tree,n=>n.type==='div'&&typeof n.props.onScroll==='function')[0];
 try{
  let tree=h.render();scroll(tree).props.ref.current=node;h.effects();node.scrollTop=460;scroll(tree).props.onScroll({currentTarget:node});
  tree=h.render({...props,active:false});h.effects();node.scrollTop=0;scroll(tree).props.onScroll({currentTarget:node});
  tree=h.render(props);h.effects();assert.equal(node.scrollTop,460);
  tree=h.render({...props,context:{local:{...context.local,sourceId:'disk'}}});h.effects();assert.equal(node.scrollTop,0);
 }finally{h.close();}
});

test("album card has sibling native Play/navigation buttons, translated focus overlay and one explicit activation",async()=>{
  for(const language of ["en","pt-BR"]){
    globalThis.__exploreTranslation=translations[language];const c=context(),s=createExploreSession();s.syncContext(c,true);
    const entry=s.visibleEntries()[0],d=deferred();let plays=0,opens=0,valid;
    const props={entry,revision:s.getSnapshot().revision,active:true,current:()=>true,isPlaybackAvailable:true,
      onPlay:async(e,current)=>{assert.equal(e,entry);plays++;valid=current;await d.promise;},onOpen(){opens++;}};
    const h=host(ExploreAlbumCard,props);let tree=h.render();h.effects();
    const buttons=nodes(tree,n=>n.type==="button");assert.equal(buttons.length,2);
    for(const b of buttons)assert.equal(nodes(b.props.children,n=>n.type==="button").length,0);
    const [open,play]=buttons;
    assert.equal(play.props.type,"button");assert.equal(play.props.onKeyDown,undefined,"native Enter/Space activation, no duplicate key handler");
    assert.equal(play.props.title,translations[language].t("common.playAlbum"));assert.ok(play.props["aria-label"].includes(entry.album.title));
    assert.ok(nodes(tree,n=>n.props?.className?.includes("bg-black/40")&&n.props.className.includes("group-focus-within:opacity-100")).length);
    assert.ok(play.props.className.includes("w-12 h-12"));assert.equal(plays,0,"render/effects never play");
    for(const key of [" ","Enter"]){let stopped=0;tree.props.onKeyDown({key,stopPropagation(){stopped++;},preventDefault(){assert.fail("native activation must remain enabled");}});assert.equal(stopped,1);assert.equal(plays,0);}
    const pending=play.props.onClick();await play.props.onClick();assert.equal(plays,1);assert.equal(opens,0);assert.equal(valid(),true);
    tree=h.render();h.effects();assert.equal(nodes(tree,n=>n.type==="button")[1].props.disabled,true);
    d.resolve();await pending;tree=h.render();h.effects();assert.equal(nodes(tree,n=>n.type==="button")[1].props.disabled,false);
    open.props.onClick();assert.equal(opens,1);assert.equal(plays,1);
    const artwork=host(ExploreAlbumCard,{...props,artworkOnly:true});assert.equal(nodes(artwork.render(),n=>n.type==="button").length,0);artwork.close();
    h.close();
  }
});

test("card invalidates delayed playback after revision, visibility, MPD, update or unmount even after returning",async()=>{
  globalThis.__exploreTranslation=translations.en;
  for(const change of [{revision:2},{active:false},{isPlaybackAvailable:false},{artworkBlocked:true},"unmount"]){
    const c=context(),s=createExploreSession();s.syncContext(c,true);const entry=s.visibleEntries()[0],d=deferred();let submitted=0;
    const props={entry,revision:1,active:true,isPlaybackAvailable:true,current:()=>true,onOpen(){},onPlay:async(_,valid)=>{await d.promise;if(valid())submitted++;}};
    const h=host(ExploreAlbumCard,props);let tree=h.render();h.effects();const run=nodes(tree,n=>n.type==="button")[1].props.onClick();
    if(change==="unmount")h.close();else{h.render({...props,...change});h.effects();h.render(props);h.effects();}
    d.resolve();await run;assert.equal(submitted,0);
    if(change!=="unmount"){tree=h.render();assert.equal(nodes(tree,n=>n.type==="button")[1].props.disabled,false);h.close();}
  }
});

test("card failures use sanitized Local playback diagnostics and translated feedback; an explicit retry succeeds",async()=>{
  globalThis.__exploreTranslation=translations.en;const old=console.error,logs=[];console.error=(...args)=>logs.push(args);
  try{
    const c=context(),s=createExploreSession();s.syncContext(c,true);let fail=true;
    const props={entry:s.visibleEntries()[0],revision:1,active:true,isPlaybackAvailable:true,current:()=>true,onOpen(){},onPlay:async()=>{if(fail)throw Error("/private/fixture?X-Plex-Token=secret");}};
    const h=host(ExploreAlbumCard,props);let tree=h.render();h.effects();await nodes(tree,n=>n.type==="button")[1].props.onClick();tree=h.render();
    assert.ok(nodes(tree,n=>n.props?.role==="alert").some(n=>n.props.children===translations.en.t("explore.albumPlayError")));
    assert.equal(logs.length,1);assert.doesNotMatch(JSON.stringify(logs),/private|secret|X-Plex/);
    fail=false;await nodes(tree,n=>n.type==="button")[1].props.onClick();tree=h.render();assert.equal(nodes(tree,n=>n.props?.role==="alert").length,0);h.close();
  }finally{console.error=old;}
});

test("all real suggested-album uses including themes, artist slices and virtual genre items receive Play; portraits and skeletons do not",()=>{
  globalThis.__exploreTranslation=translations.en;const c=context(0);c.local.albums=Array.from({length:30},(_,i)=>({...local(`play-${i}`,"Artist"),title:`Album ${i}`,year:"1995",genres:["Jazz"]}));
  const play=async()=>{};const props={context:c,active:true,isPlaybackAvailable:true,onPlay:play,ensureLocal(){},isCurrent:()=>true,onOpen(){}};
  const h=host(ExploreView,props);let tree=h.render();h.effects();
  const cards=nodes(tree,n=>n.type===ExploreAlbumCard);assert.ok(cards.length>10,"main and theme shelves");
  const check=card=>{assert.equal(card.props.onPlay,play);assert.equal(card.props.isPlaybackAvailable,true);assert.equal(card.props.current(),true);};cards.forEach(check);
  const session=h.slots[0].value,state=session.getSnapshot();session.openSlice(state.artists[0],state.revision);tree=h.render();h.effects();nodes(tree,n=>n.type===ExploreAlbumCard).forEach(check);
  session.leaveSlice(session.getSnapshot().revision);session.openSlice(session.getSnapshot().genres[0],session.getSnapshot().revision);tree=h.render();h.effects();
  const slice=nodes(tree,n=>n.type===ExploreGenreSlice)[0];assert.ok(slice);assert.ok(slice.props.entries.length>6);slice.props.entries.forEach(entry=>check(slice.props.renderItem(entry)));
  h.close();
  const loading=host(ExploreView,{...props,context:{local:{albums:[],state:"loading"}}});assert.equal(nodes(loading.render(),n=>n.type===ExploreAlbumCard).length,0);loading.close();
  const ast=ts.createSourceFile("ExploreView.tsx",source("../components/ExploreView.tsx"),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let real=0,portrait=0;
  const visit=n=>{if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==="ExploreAlbumCard"){
    const attrs=n.attributes.properties.map(p=>p.name?.getText(ast));if(attrs.includes("artworkOnly")){portrait++;assert.ok(!attrs.includes("onPlay"));}else{real++;assert.ok(attrs.includes("onPlay"));}
  }ts.forEachChild(n,visit);};visit(ast);assert.equal(real,3);assert.equal(portrait,1);
});

test("actual App playback callback revalidates live catalog, source scope, visibility, saves and MPD before submission",async()=>{
  for(const mode of ["success","mpd","loading","saveBlocked","mpdBusy","error","loaded","removed","version","scope","unavailable","hidden","card","save","save-generation","cancel-return"]){
    const c=context(),s=createExploreSession();s.syncContext(c,true);const entry=s.visibleEntries()[0],d=deferred();let plays=0,card=true;
    const live={current:{context:c,visible:true}},available={current:true},saving={current:false},saveGeneration={current:0};
    let snapshot={albums:c.local.albums,loaded:true,loading:false,saveBlocked:false,mpdBusy:false,error:null};
    const playback=createLocalAlbumPlayback(()=>d.promise,async()=>null,async()=>{plays++;});
    const scope={localAlbumCatalog:{getSnapshot:()=>snapshot},exploreLive:live,playbackAvailableRef:available,
      configSaveInProgressRef:saving,configSaveGenerationRef:saveGeneration,explorePlayback:playback,
      exploreEntryCurrent:e=>exploreEntryIsCurrent(live.current.context,e)};
    const run=new Function(...Object.keys(scope),`return (${expression("playExploreAlbum")});`)(...Object.values(scope));
    const pending=run(entry,()=>card);
    if(mode==="mpd")available.current=false;
    else if(["loading","saveBlocked","mpdBusy"].includes(mode))snapshot={...snapshot,[mode]:true};
    else if(mode==="loaded")snapshot={...snapshot,loaded:false};
    else if(mode==="error")snapshot={...snapshot,error:"fixture"};
    else if(mode==="removed")snapshot={...snapshot,albums:[]};
    else if(mode==="version")snapshot={...snapshot,albums:[...snapshot.albums]};
    else if(mode==="scope")live.current.context={local:{...c.local,albums:[...c.local.albums],sourceId:"other"}};
    else if(mode==="unavailable")live.current.context={local:{...c.local,state:"unavailable"}};
    else if(mode==="hidden")live.current.visible=false;
    else if(mode==="card")card=false;
    else if(mode==="save")saving.current=true;
    else if(mode==="save-generation")saveGeneration.current++;
    else if(mode==="cancel-return"){
      let effect;
      const visit=n=>{if(ts.isCallExpression(n)&&n.expression.getText(appAst)==="useLayoutEffect"&&n.arguments[0].getText(appAst).includes("explorePlayback.cancel"))effect=n;ts.forEachChild(n,visit);};visit(appAst);assert.ok(effect);
      const deps=effect.arguments[1].getText(appAst);for(const key of ["exploreAlbums","activeExploreSourceId","centralSection","playerTarget","exploreDetail","isPlaybackAvailable","playbackStatus.is_updating","localCatalog.albums","localCatalog.loading","localCatalog.saveBlocked"])assert.ok(deps.includes(key));
      const setup=new Function("explorePlayback",`${compile("const setup = "+effect.arguments[0].getText(appAst))}; return setup;`)(playback);
      setup()();
    }
    d.resolve([{disc:{number:1},tracks:[{path:"exact-source/disc.cue/track0001",name:"one"}]}]);await pending;
    assert.equal(plays,mode==="success"?1:0,mode);
  }
});
