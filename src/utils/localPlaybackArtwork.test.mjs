import { sourceEnabled } from "./musicSources.ts";
import { LocalPlaybackThumbnail } from "./localPlaybackThumbnail.ts";
import { createLocalCoverBatch } from "./localCoverBatch.ts";
import { createLocalSourceReadQueue } from "./localSourceReadQueue.ts";
import * as playbackIntents from "./playbackIntent.ts";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import { LocalArtworkResolver, artworkReadBlocked } from "./localArtwork.ts";
import { LocalAlbumCatalog } from "./localAlbumCatalog.ts";
import { currentMediaKey, localAlbumForMedia } from "./playerNavigation.ts";
import { logError } from "./frontendDiagnostics.ts";
const coverPaths = calls => calls.flatMap(c=>c[0]==="get_local_covers"?c[1].paths:c[0]==="get_local_cover"?[c[1].path]:[]);
const read = file => readFileSync(new URL(`../${file}`, import.meta.url), "utf8");
const compile = (file, deps) => {
  const js = ts.transpileModule(read(file), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
  const exports = {}; new Function("require", "exports", js)(id => { if(id === "../utils/playbackIntent") return playbackIntents; if(id==="./config")return {ensureDirectOutputChoice:async()=>{}}; if(id==="../utils/localCoverBatch")return {createLocalCoverBatch}; if(id==="../utils/localSourceReadQueue")return {createLocalSourceReadQueue,localSourceReadQueue:createLocalSourceReadQueue()}; assert.ok(id in deps, id); return deps[id]; }, exports); return exports;
};
const appSource = read("App.tsx"), ast = ts.createSourceFile("App.tsx", appSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let projection, snapshotEffect;
const visit = node => {
  if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "presentationPlayback") projection = node.initializer.getText(ast);
  if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect" && node.arguments[0]?.getText(ast).includes("nowPlayingSnapshotRef.current =")) snapshotEffect = node.arguments[0].getText(ast);
  ts.forEachChild(node, visit);
}; visit(ast); assert.ok(projection && snapshotEffect);
const album = (id, folder) => ({ id, folder_path: folder, title: id, artist: "Fixture", source_id: folder.split("/")[0], discs: [], track_count: 1 });
const albums = [album("first", "Source/Album"), album("second", "Other/Album")];
const raw = () => ({ current_media: { kind: "local", uri: "Source/Album/one.flac", queue_index: 0 }, title: "One", artist: "Fixture", album: "first", state: "pause", elapsed: 19.5, duration: 99, is_updating: false, thumb: null, plex_image: null });
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => {resolve=a;reject=b;}); return {promise,resolve,reject}; };
const flush = async () => { for(let i=0;i<30;i++) await Promise.resolve(); };
function mount({ cover = async () => "local-cover", cached = async () => null, inventory = albums, online = true, presentation, cardCover = () => null, thumbnail = new LocalPlaybackThumbnail(() => ({ getItem: () => null })) } = {}) {
  const calls = [], slots = [], effects = []; let cursor = 0, disposed = false, late = 0;
  const react = {
    useState(initial) { const i=cursor++; if(!(i in slots)) slots[i]=initial; return [slots[i], value=>{if(disposed)late++;slots[i]=value;}]; },
    useEffect(fn,deps) { const i=cursor++,old=slots[i]; if(!old||deps.some((d,n)=>!Object.is(d,old.deps[n])))effects.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()};}); },
  };
  react.useLayoutEffect=react.useEffect;
  const audioService = compile("services/audio.ts", {
    "@tauri-apps/api/core": { invoke: async (cmd,args) => {
      calls.push([cmd,args]);
      if(cmd==="get_local_albums")return inventory;
      if(cmd==="get_local_cover")return cover(args.path);
      if(cmd==="get_local_covers")return Promise.all(args.paths.map(async path=>{try{return {Ok:await cover(path)}}catch(error){return {Err:String(error)}}}));
      if(cmd==="get_online_album_cover"){assert.equal(args.cacheOnly,true,"recovery must never permit HTTP lookup");return cached(args.request);}
      assert.fail(`Unexpected audio/playback command ${cmd}`);
    } }, "../utils/frontendDiagnostics": { diagnosticInvoke: (invoke,cmd,args)=>invoke(cmd,args) },
  }).audioService;
  const resolver = new LocalArtworkResolver(audioService.getLocalCover, async()=>assert.fail("no online search"));
  const catalog = new LocalAlbumCatalog(audioService.getLocalAlbums,
    presentation ? { read: () => presentation, write() {} } : undefined);
  if (presentation) catalog.setPresentationContext(["/music"]);
  const hook = compile("utils/useLocalPlaybackArtwork.ts", {
    react, "./localPlaybackThumbnail": { localPlaybackThumbnail: thumbnail }, "./playerNavigation": {currentMediaKey,localAlbumForMedia}, "./localArtwork": {artworkReadBlocked},
    "./localArtworkSession": {localArtworkResolver:resolver,localAlbumThumbnails:{get:cardCover}}, "../services/audio": {audioService}, "./frontendDiagnostics": {logError},
  }).useLocalPlaybackArtwork;
  const ensure = ()=>catalog.ensureLoaded(false,true);
  const f = { status:raw(),available:true,showSettings:false,showAbout:false,online,catalog,calls,resolver,
    render() {
      cursor=0;const state=catalog.getSnapshot();resolver.setContext(artworkReadBlocked(f.status.is_updating,state),state.albums);
      const vars={sourceEnabled,useLocalPlaybackArtwork:hook,playbackStatus:f.status,sourceArtworkPlayback:f.status,localCatalog:state,isPlaybackAvailable:f.available,
        config:{first_run:false,online_artwork_enabled:f.online},showSettings:f.showSettings,showAbout:f.showAbout,ensurePlaybackArtworkCatalog:ensure};
      f.display=new Function(...Object.keys(vars),`return (${projection});`)(...Object.values(vars));
      effects.splice(0).forEach(run=>run());return f.display;
    },
    dispose(){slots.forEach(s=>s?.cleanup?.());disposed=true;},late:()=>late,
  }; return f;
}
async function ready(f){f.render();await flush();f.render();await flush();return f.render();}

test("actual App projection restores Local artwork before Play through focal IPC without changing cursor or polling data",async()=>{
  const f=mount();const original=structuredClone(f.status);const display=await ready(f);
  assert.equal(display.thumb,"local-cover");assert.deepEqual(f.status,original);
  assert.deepEqual({...display,thumb:null},original);assert.equal(display.elapsed,19.5);assert.equal(display.state,"pause");
  assert.equal(f.calls.filter(c=>c[0]==="get_local_albums").length,0);assert.equal(coverPaths(f.calls).length,1);
  for(let i=0;i<20;i++){f.status={...f.status,elapsed:19.5+i};f.render();await flush();}
  assert.equal(f.calls.length,1,"health/status rerenders perform no artwork or playback I/O");
  const sourceInfo={media:f.display.current_media,info:{provenance:"local",codec:"FLAC",compression:"lossless",sample_type:"integer",sample_rate:96000,bit_depth:24,dsd_level:null,revision:"fixture"}};
  const ref={current:null},published=[];const vars={sourceEnabled,sourceInfo,queueArtwork:{blocked:false,onlineEnabled:true,generation:0},nowPlayingSnapshotRef:ref,configuredSignalRoute:()=>({mode:"shared"}),config:{},mpdHealth:{state:"available"},presentationPlayback:f.display,publishCurrentNowPlayingSnapshot:()=>published.push(ref.current)};
  new Function(...Object.keys(vars),`return (${snapshotEffect});`)(...Object.values(vars))();
  assert.equal(published[0].playback,f.display,"actual detached-window bridge publishes the projection");
  assert.equal(published[0].queueArtwork,vars.queueArtwork,"the same bridge carries the queue artwork gate/generation");
  assert.equal(published[0].sourceInfo,sourceInfo,"the same bridge carries the source facts without new reads");
  for(const tag of ["PlayerBar","QueueDrawer","NowPlayingHost"]){
    let found;const walk=n=>{if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)===tag)found=n;ts.forEachChild(n,walk);};walk(ast);assert.ok(found);
    const attr=found.attributes.properties.find(p=>p.name?.getText(ast)===(tag==="NowPlayingHost"?"snapshot":"status")).initializer.expression.getText(ast);
    assert.ok(attr.includes("presentationPlayback"),`${tag} receives projected artwork`);
  }
  f.dispose();
});

test("CUE/disc references use the existing Local album artwork contract and persisted enrichment is cache-only",async()=>{
  const cueAlbum={...albums[0],discs:[{folder_path:"Source/Album/image.cue",number:1}]};
  const f=mount({inventory:[cueAlbum],cover:async path=>path.endsWith("image.cue")?"embedded-cover":null});f.status.current_media.uri="Source/Album/image.cue/track0001";
  assert.equal((await ready(f)).thumb,"embedded-cover");assert.deepEqual(coverPaths(f.calls),["Source/Album","Source/Album/image.cue"]);f.dispose();
  const cached=mount({cover:async()=>null,cached:async request=>{assert.equal(request.album_id,"first");return "previous-enrichment";}});
  assert.equal((await ready(cached)).thumb,"previous-enrichment");assert.equal(cached.calls.filter(c=>c[0]==="get_online_album_cover").length,1);cached.dispose();
});

test("missing/error artwork stays a placeholder without polling retries; disabled enrichment never queries cache",async()=>{
  for(const fail of [false,true]){
    const old=console.error,logs=[];console.error=(...v)=>logs.push(v);
    try{const f=mount({cover:async()=>{if(fail)throw Error("/private/path?token=secret");return null;}});assert.equal((await ready(f)).thumb,null);
      const count=f.calls.length;for(let i=0;i<10;i++){f.status={...f.status,elapsed:i};f.render();await flush();}assert.equal(f.calls.length,count);
      assert.doesNotMatch(JSON.stringify(logs),/private|secret/);f.dispose();
    }finally{console.error=old;}
  }
  const f=mount({cover:async()=>null,online:false});await ready(f);assert.equal(f.calls.filter(c=>c[0]==="get_online_album_cover").length,0);f.dispose();
});

test("late artwork is discarded for track/source changes, modal close, catalog generation or unmount; Plex remains unchanged",async()=>{
  for(const mode of ["track","plex","cached-plex","existing","close","catalog","unmount"]){
    const d=deferred();let firstReads=0;const f=mount(mode==="cached-plex"?{cover:async()=>null,cached:async()=>d.promise}:{cover:async path=>path==="Source/Album"?(++firstReads===1?d.promise:"fresh-generation-cover"):"second-cover"});await ready(f);
    if(mode==="track")f.status={...f.status,current_media:{kind:"local",uri:"Other/Album/two.flac",queue_index:1},title:"Two"};
    else if(mode==="plex"||mode==="cached-plex")f.status={...f.status,current_media:{kind:"plex",server_id:"server",part_key:"part",queue_index:1},plex_image:{server_id:"server",path:"/library/metadata/2/thumb"}};
    else if(mode==="existing")f.status={...f.status,thumb:"authoritative-thumb"};
    else if(mode==="close")f.showSettings=true;
    else if(mode==="catalog")f.catalog.requestRefresh();
    else f.dispose();
    if(mode!=="unmount"){f.render();await flush();f.render();}
    d.resolve("old-cover");await flush();
    if(mode!=="unmount"){const result=f.render();assert.notEqual(result.thumb,"old-cover",mode);if(mode==="track")assert.equal(result.thumb,"second-cover");if(mode==="plex"||mode==="cached-plex"||mode==="existing")assert.equal(result,f.status);f.dispose();}
    assert.equal(f.late(),0);
  }
});

test("Save/indexing gates delay and then resume artwork without requiring playback or a new identity",async()=>{
  for(const gate of ["save","unavailable","inactive"]){
    const f=mount();if(gate==="save")f.catalog.setSaveBlocked(true);else if(gate==="unavailable")f.available=false;else f.showSettings=true;
    await ready(f);assert.equal(f.calls.length,0,gate);
    f.catalog.setSaveBlocked(false);f.available=true;f.showSettings=false;await ready(f);assert.equal(f.display.thumb,"local-cover");f.dispose();
  }
  const d=deferred();let reads=0;const g=mount({cover:async()=>++reads===1?d.promise:"fresh-cover"});await ready(g);
  g.status={...g.status,is_updating:true};g.render();d.resolve("stale-cover");await flush();assert.equal(g.render().thumb,null);
  g.status={...g.status,is_updating:false};await ready(g);assert.equal(g.display.thumb,"fresh-cover");assert.equal(reads,2);g.dispose();
});

test("already displayed paused artwork survives save/index gates without additional reads; changed media never inherits it",async()=>{
 for(const gate of ['save','index']){
  const f=mount();await ready(f);const reads=f.calls.length;
  if(gate==='save')f.catalog.setSaveBlocked(true);else f.status={...f.status,is_updating:true};
  assert.equal((await ready(f)).thumb,'local-cover',gate);
  assert.equal(f.calls.length,reads,'blocking still prevents new artwork reads');
  assert.equal(f.display.state,'pause');assert.equal(f.display.elapsed,19.5);
  f.status={...f.status,current_media:{kind:'local',uri:'Other/Album/two.flac',queue_index:1},title:'Two'};
  assert.equal(f.render().thumb,null,'never reuse another track artwork while blocked');f.dispose();
 }
});


test("restored player artwork does not request the album collection before Play",async()=>{
  const f=mount();const before=structuredClone(f.status);await ready(f);
  assert.equal(f.display.thumb,'local-cover');
  assert.equal(f.calls.filter(c=>c[0]==='get_local_albums').length,0);
  assert.deepEqual(coverPaths(f.calls),['Source/Album']);
  assert.deepEqual(f.status,before,'artwork cannot mutate queue, position or playback');
  f.dispose();
});


test("startup queues the focal cover before the browser catalog, and malformed media cannot request a path",async()=>{
  const pending=deferred();const f=mount({inventory:pending.promise});f.render();
  // The player layout effect runs before the browser's passive ensureLoaded.
  f.catalog.ensureLoaded(false,true);assert.equal(f.catalog.getSnapshot().loading,true);
  await flush();f.render();await flush();assert.equal(f.render().thumb,'local-cover');assert.equal(f.catalog.getSnapshot().loaded,false);
  assert.deepEqual(f.calls.map(c=>c[0]),['get_local_cover','get_local_albums']);f.dispose();pending.resolve(albums);
  for(const uri of ['../track.flac','Source/../track.flac','Source//track.flac','Source/Album/track.flac\n','https://fixture.invalid/music','/Source/Album/track.flac']){
    const g=mount();g.status.current_media.uri=uri;await ready(g);assert.equal(g.calls.length,0,uri);g.dispose();
  }
});


test("a restored presentation thumbnail paints in the first player render while focal IPC remains pending", async () => {
  const cover = "data:image/jpeg;base64," + Buffer.from([255,216,255,192,0,11,8,0,80,0,80,1,1,17,0,255,217]).toString("base64");
  let value = null;
  const storage = { getItem: () => value, setItem: (_key, next) => { value = next; } };
  const previous = new LocalPlaybackThumbnail(() => storage, () => cover);
  previous.setContext(["/music"]);
  previous.capture(raw().current_media, { src: "data:image/png;base64,AAAA" }, () => true);
  const thumbnail = new LocalPlaybackThumbnail(() => storage);
  thumbnail.setContext(["/music"]);
  const pending = deferred();
  const f = mount({ thumbnail, cover: () => pending.promise });
  const original = structuredClone(f.status);
  assert.equal(f.render().thumb, cover, "no await, catalog, filesystem or MPD response required to paint");
  assert.deepEqual(f.status, original);
  await flush(); assert.equal(f.render().thumb, cover);
  assert.deepEqual(f.calls.map(c => c[0]), ["get_local_cover"], "preview never suppresses the validated lookup");
  pending.resolve("fresh-full-cover"); await flush();
  assert.equal(f.render().thumb, "fresh-full-cover");
  f.status = { ...f.status, current_media: { kind: "local", uri: "Other/Album/two.flac", queue_index: 1 } };
  assert.notEqual(f.render().thumb, cover, "changed media cannot inherit the preview");
  f.dispose();
});


test("an existing browser thumbnail can preview the restored player before the new single-entry cache is warmed",async()=>{
 const pending=deferred();
 const f=mount({presentation:albums,cardCover:album=>album.id==='first'?'saved-card-preview':null,cover:()=>pending.promise});
 const state=f.catalog.getSnapshot();assert.equal(state.loaded,false);assert.equal(state.albums.length,0);
 assert.equal(f.render().thumb,'saved-card-preview');await flush();
 assert.deepEqual(f.calls.map(c=>c[0]),['get_local_cover']);
 assert.equal(f.catalog.getSnapshot().loaded,false,'preview never confirms the saved catalog');
 pending.resolve('current-full-cover');await flush();assert.equal(f.render().thumb,'current-full-cover');f.dispose();
});


test("a changed Local track obtains its cover during a bound Check, while Preparing remains suspended", async () => {
  const f = mount(); f.catalog.ensureLoaded(false, true); await flush();
  f.status = { ...f.status, is_updating: true };
  f.catalog.onMpdStatus(true, true, false); await ready(f);
  assert.deepEqual(coverPaths(f.calls), []);
  const before = structuredClone(f.status);
  f.catalog.onMpdStatus(true, true, true); await ready(f);
  assert.equal(f.display.thumb, "local-cover");
  assert.deepEqual(coverPaths(f.calls), ["Source/Album"]);
  assert.deepEqual(f.status, before, "artwork must not change raw playback state or queue");
  f.dispose();
});
