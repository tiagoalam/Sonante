import { NowPlayingFullscreen } from "./nowPlayingFullscreen.ts";
import * as layoutHelpers from "./nowPlayingLayout.ts";
import { passiveQueueInsertion } from "../../tests/support/passiveQueueInsertion.mjs";
import * as queuePresentationHelpers from "./queuePresentation.ts";
import { createLocalCoverBatch } from "./localCoverBatch.ts";
import { createLocalSourceReadQueue } from "./localSourceReadQueue.ts";
import * as signalPathHelpers from "./signalPath.ts";
import * as playbackIntents from "./playbackIntent.ts";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import * as presentation from "./nowPlayingPresentation.ts";
import { LocalArtworkResolver, artworkReadBlocked } from "./localArtwork.ts";
import { LocalAlbumCatalog } from "./localAlbumCatalog.ts";
import { VisibleQueueArtwork } from "./visibleQueueArtwork.ts";
import { localAlbumForMedia } from "./playerNavigation.ts";
import { logError } from "./frontendDiagnostics.ts";
const coverPaths = calls => calls.filter(c=>c[0]==="get_local_covers").flatMap(c=>c[1].paths);
const read = file => readFileSync(new URL(`../${file}`,import.meta.url),"utf8");
function compile(file,deps){const js=ts.transpileModule(read(file),{fileName:file,compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const exports={};new Function("require","exports",js)(id=>{if(id === "../utils/nowPlayingFullscreen")return {NowPlayingFullscreen}; if(id === "../utils/playbackIntent") return playbackIntents; if(id==="./config")return {ensureDirectOutputChoice:async()=>{}}; if(id==="../utils/signalPath")return signalPathHelpers; if(id==="../utils/localCoverBatch")return {createLocalCoverBatch}; if(id==="../utils/localSourceReadQueue")return {createLocalSourceReadQueue,localSourceReadQueue:createLocalSourceReadQueue()}; assert.ok(id in deps,id);return deps[id];},exports);return exports;}
const flush=async()=>{for(let i=0;i<60;i++)await Promise.resolve();};
const defer=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve};};
const album=(id,folder)=>({id,title:"Same title",artist:"Same artist",folder_path:folder,discs:[],source_id:folder.split("/")[0],track_count:2});
const a=album("a","Source/Album"),b=album("b","Other/Album");
const track=(uri)=>({title:"Same title",artist:"Same artist",album:"Same title",uri,media_locator:{kind:"local",uri}});
const common={"../utils/nowPlayingLayout":layoutHelpers,...passiveQueueInsertion,"../utils/queuePresentation":queuePresentationHelpers,"react/jsx-runtime":jsx,"lucide-react":icons,"react-i18next":{useTranslation:()=>({t:key=>key})},"./nowPlaying.css":{}};
const {TrackArtwork}=compile("components/TrackArtwork.tsx",{...common,react:{...React,default:React},"./PlexImage":{PlexImage:()=>null}});
const nodes=(tree,pred)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,pred)):!tree||typeof tree!=="object"?[]:[...(pred(tree)?[tree]:[]),...nodes(tree.props?.children,pred)];
const fixtures=new Set();
test.afterEach(()=>{for(const f of fixtures)f.dispose();fixtures.clear();});
function mount({items=[track("Source/Album/1.flac"),track("Source/Album/2.flac"),track("Other/Album/3.flac")],inventory=[a,b],cover=async path=>`cover:${path}`,cache=async()=>null,detached=false}={}){
 const calls=[],slots=[],effects=[],layouts=[];let cursor=0,disposed=false,late=0,reader;
 const effect=(list,fn,deps)=>{const i=cursor++,old=slots[i];if(!old||deps.some((v,n)=>!Object.is(v,old.deps[n])))list.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()};});};
 const react={...React,default:React,useId:()=> "queue-filter-test",
  useState(initial){const i=cursor++;if(!(i in slots))slots[i]=initial;return[slots[i],value=>{if(disposed)late++;slots[i]=typeof value==="function"?value(slots[i]):value;}];},
  useRef(initial){const i=cursor++;return slots[i]??(slots[i]={current:initial});},
  useMemo(fn,deps){const i=cursor++,old=slots[i];if(!old||deps.some((v,n)=>!Object.is(v,old.deps[n])))slots[i]={deps,value:fn()};return slots[i].value;},
  useEffect(fn,deps){effect(effects,fn,deps);},useLayoutEffect(fn,deps){effect(layouts,fn,deps);},useSyncExternalStore(_subscribe,get){return get();},
 };
 const f={items,inventory,props:{open:true,available:true,detached,artworkContext:{blocked:false,onlineEnabled:true,generation:0},status:{volume:{available:true,writable:true,value:50,muted:false},is_updating:false,current_media:{kind:"local",uri:items[0]?.uri??"",queue_index:0},thumb:"current-cover"}},calls,
  render(){cursor=0;if(!f.props.detached)resolver.setContext(artworkReadBlocked(!!f.props.status?.is_updating,catalog.getSnapshot()),catalog.getSnapshot().albums);f.tree=Queue(f.props);layouts.splice(0).forEach(run=>run());effects.splice(0).forEach(run=>run());return f.tree;},
  async ready(){for(let i=0;i<7;i++){f.render();await flush();}return f.render();},
  async replace(next){f.items=next;reader.refresh();await flush();await f.ready();},
  scroll(top){nodes(f.tree,n=>n.props?.className==="np-queue-scroll")[0].props.onScroll({currentTarget:{scrollTop:top,clientHeight:128}});},
  thumbs(){return nodes(f.tree,n=>n.type===TrackArtwork).map(n=>n.props.track.thumb??null);},
  dispose(){slots.forEach(s=>s?.cleanup?.());disposed=true;},late:()=>late,
 };
 const {audioService}=compile("services/audio.ts",{"@tauri-apps/api/core":{invoke:async(cmd,args)=>{calls.push([cmd,args]);if(cmd==="get_queue")return f.items.map(t=>({...t}));if(cmd==="get_local_albums")return f.inventory;if(cmd==="get_local_covers")return Promise.all(args.paths.map(async path=>{try{return {Ok:await cover(path)}}catch(error){return {Err:String(error)}}}));if(cmd==="get_online_album_cover"){assert.equal(args.cacheOnly,true);return cache(args.request);}assert.fail(`unexpected transport/HTTP ${cmd}`);}},"../utils/frontendDiagnostics":{diagnosticInvoke:(invoke,cmd,args)=>invoke(cmd,args)}});
 const catalog=new LocalAlbumCatalog(audioService.getLocalAlbums),resolver=new LocalArtworkResolver(audioService.getLocalCover,async()=>assert.fail("no HTTP"));
 const hook=compile("utils/useLocalQueueArtwork.ts",{react,"./localAlbumCatalogSession":{localAlbumCatalog:catalog},"./localArtworkSession":{localArtworkResolver:resolver},"./localArtwork":{artworkReadBlocked},"./playerNavigation":{localAlbumForMedia},"./nowPlayingPresentation":presentation,"./visibleQueueArtwork":{VisibleQueueArtwork},"../services/audio":{audioService},"./frontendDiagnostics":{logError}}).useLocalQueueArtwork;
 class Reader extends presentation.QueuePreviewReader{constructor(load,publish){super(load,publish,100000);reader=this;}}
 const Queue=compile("components/NowPlayingQueue.tsx",{...common,react,"../utils/useLocalQueueArtwork":{useLocalQueueArtwork:hook},"../services/audio":{audioService},"../utils/nowPlayingPresentation":{...presentation,QueuePreviewReader:Reader},"./TrackArtwork":{TrackArtwork}}).NowPlayingQueue;
 Object.assign(f,{catalog,resolver,Queue});fixtures.add(f);return f;
}

test("actual virtual queue restores noncurrent rows, shares one album read and preserves logical items/current artwork",async()=>{
 const f=mount(),before=structuredClone(f.items);await f.ready();assert.deepEqual(f.thumbs(),["current-cover","cover:Source/Album","cover:Other/Album"]);assert.deepEqual(f.items,before);
 const imageReads=f.calls.filter(c=>c[0]!=="get_queue").length;await f.replace(f.items.map(item=>({...item})));assert.equal(f.calls.filter(c=>c[0]!=="get_queue").length,imageReads,"identical queue polling does not retry images");
 assert.deepEqual(coverPaths(f.calls),["Source/Album","Other/Album"]);
 const artwork=nodes(f.tree,n=>n.type===TrackArtwork)[1];assert.match(renderToStaticMarkup(React.createElement(TrackArtwork,artwork.props)),/src="cover:Source\/Album"/);
 const count=f.calls.length;for(let i=0;i<20;i++){f.props.status={...f.props.status,elapsed:i};await f.ready();}assert.equal(f.calls.length,count);f.dispose();
});

test("visible plus overscan only, bounded concurrency through local/cache fallback, scrolling and polling reuse",async()=>{
 const inventory=Array.from({length:50},(_,i)=>album(`a${i}`,`S/Album${i}`));const pending=[];let running=0,max=0;
 const f=mount({items:inventory.map(x=>track(`${x.folder_path}/1.flac`)),inventory,cover:async()=>{running++;max=Math.max(max,running);const d=defer();pending.push(d);await d.promise;running--;return null;},cache:async request=>`cached:${request.album_id}`});
 await f.ready();assert.equal(pending.length,2);assert.equal(coverPaths(f.calls).length,2);
 for(let i=0;i<8;i++){pending.splice(0).forEach(d=>d.resolve());await f.ready();}
 assert.equal(max,2);assert.equal(coverPaths(f.calls).length,15,"16 visible/overscan rows: current cover reused, 15 lookups, not 50");
 f.scroll(32*64);await f.ready();assert.equal(pending.length,2);pending.splice(0).forEach(d=>d.resolve());await f.ready();
 assert.ok(coverPaths(f.calls).includes("S/Album28"));assert.ok(!coverPaths(f.calls).includes("S/Album49"));
 f.dispose();pending.splice(0).forEach(d=>d.resolve());await flush();assert.equal(f.late(),0);
});

test("exact source/disc CUE matching excludes Plex, unknown URI and ambiguity despite identical titles",async()=>{
 const cue={...a,discs:[{folder_path:"Source/Album/image.cue",number:1}]};const plex={...track("Source/Album/image.cue/track0001"),media_locator:{kind:"plex",server_id:"s",part_key:"p"},plex_image:{server_id:"s",path:"thumb"}};
 const f=mount({inventory:[cue,b],items:[track("Source/Album/image.cue/track0001"),track("Source/Album/image.cue/track0002"),track("Other/Album/x.flac"),plex,track("Source/Album-extra/x.flac"),track("http://server/x")],cover:async path=>path.endsWith("image.cue")?"embedded":path.startsWith("Other/")?"other-source":null});
 await f.ready();assert.deepEqual(f.thumbs(),["current-cover","embedded","other-source",null,null,null]);assert.equal(coverPaths(f.calls).length,3);assert.deepEqual(nodes(f.tree,n=>n.type===TrackArtwork)[3].props.track.plex_image,plex.plex_image);f.dispose();
 const ambiguous=mount({inventory:[a,{...a,id:"ambiguous"}],items:[track("Source/Album/1.flac"),track("Source/Album/2.flac")]});await ambiguous.ready();assert.deepEqual(ambiguous.thumbs(),["current-cover",null]);assert.equal(coverPaths(ambiguous.calls).length,0);ambiguous.dispose();
});

test("queue replace/clear/close, catalog/source changes and unmount reject late results, including cache-only reads",async()=>{
 for(const mode of ["replace","clear","close","catalog","generation","unmount","cached-close"]){
  const d=defer();const f=mount(mode==="cached-close"?{cover:async()=>null,cache:async()=>d.promise}:{cover:async()=>d.promise});await f.ready();
  if(mode==="replace")await f.replace([track("Unknown/Album/new.flac")]);
  else if(mode==="clear")await f.replace([]);
  else if(mode==="close"||mode==="cached-close")f.props.open=false;
  else if(mode==="catalog"){f.inventory=[album("new","NewSource/Album")];f.catalog.requestRefresh();}
  else if(mode==="generation")f.props.artworkContext={...f.props.artworkContext,generation:1,blocked:true};
  else f.dispose();
  if(mode!=="unmount")await f.ready();d.resolve("stale-cover");await flush();
  if(mode!=="unmount"){await f.ready();assert.ok(!f.thumbs().includes("stale-cover"),mode);f.dispose();}assert.equal(f.late(),0,mode);
 }
});

test("Save/indexing/availability gates resume; misses/errors do not retry on polling; cache preference and Plex remain intact",async()=>{
 for(const gate of ["save","index","availability","closed"]){const f=mount();if(gate==="save")f.props.artworkContext.blocked=true;if(gate==="index")f.props.status.is_updating=true;if(gate==="availability")f.props.available=false;if(gate==="closed")f.props.open=false;await f.ready();assert.equal(f.calls.filter(c=>c[0]!=="get_queue").length,0,gate);f.props.artworkContext={...f.props.artworkContext,blocked:false};f.props.status={...f.props.status,is_updating:false};f.props.available=true;f.props.open=true;await f.ready();assert.equal(f.thumbs()[1],"cover:Source/Album");f.dispose();}
 for(const failure of [false,true]){const old=console.error,logs=[];console.error=(...x)=>logs.push(x);try{const f=mount({cover:async()=>{if(failure)throw Error("/private?token=secret");return null;}});await f.ready();const count=f.calls.length;for(let i=0;i<10;i++)await f.ready();assert.equal(f.calls.length,count);assert.equal(f.thumbs()[1],null);assert.doesNotMatch(JSON.stringify(logs),/private|secret/);f.dispose();}finally{console.error=old;}}
 const f=mount({cover:async()=>null});f.props.artworkContext.onlineEnabled=false;await f.ready();assert.equal(f.calls.filter(c=>c[0]==="get_online_album_cover").length,0);f.dispose();
 const plex=mount({detached:true,items:[{title:"Plex",media_locator:{kind:"plex",server_id:"s",part_key:"p"}}]});await plex.ready();assert.deepEqual(plex.calls.map(c=>c[0]),["get_queue"]);plex.dispose();
});

test("detached queue initializes its own empty catalog, refreshes changed generation, and respects remote gates",async()=>{
 const f=mount({detached:true});assert.equal(f.catalog.getSnapshot().loaded,false);f.props.artworkContext.blocked=true;await f.ready();assert.equal(f.calls.filter(c=>c[0]==="get_local_albums").length,0);
 f.props.artworkContext={blocked:false,onlineEnabled:true,generation:0};await f.ready();assert.equal(f.thumbs()[1],"cover:Source/Album");assert.equal(f.calls.filter(c=>c[0]==="get_local_albums").length,1);
 f.inventory=[album("new","New/Album")];f.props.artworkContext={...f.props.artworkContext,generation:1};await f.ready();assert.equal(f.thumbs()[1],null);assert.equal(f.calls.filter(c=>c[0]==="get_local_albums").length,2);f.dispose();
});

test("actual drawer/embedded/detached hosts forward artwork gates and detached identity to their queue consumer",async()=>{
 const f=mount();const react={...React,default:React,useEffect(){},useState:value=>[value,()=>{}],useRef:value=>({current:value})};
 const {QueueDrawer}=compile("components/QueueDrawer.tsx",{...common,react,"./NowPlayingQueue":{NowPlayingQueue:f.Queue}});
 const drawer=QueueDrawer({isOpen:true,onClose(){},status:f.props.status,isPlaybackAvailable:true,artworkContext:f.props.artworkContext});const row=nodes(drawer,n=>n.type===f.Queue)[0];assert.equal(row.props.artworkContext,f.props.artworkContext);f.props={...row.props};await f.ready();assert.equal(f.thumbs()[1],"cover:Source/Album");
 const {NowPlayingHost}=compile("components/NowPlayingHost.tsx",{...common,react,"./NowPlayingPresentation":{NowPlayingPresentation:()=>null},"./NowPlayingQueue":{NowPlayingQueue:f.Queue},"./PlaybackOptionsControls":{PlaybackOptionsControls:()=>null},"@tauri-apps/api/event":{},"@tauri-apps/api/window":{},"../services/audio":{audioService:{}},"../services/nowPlayingWindow":{},"../utils/frontendDiagnostics":{logError(){}},"../utils/nowPlayingHosting":{}});
 for(const embedded of [true,false]){const host=NowPlayingHost({snapshot:{playback:f.props.status,health:{state:"available"},queueArtwork:f.props.artworkContext},owner:{host:embedded?"main":"now-playing",sessionId:"s",generation:1},embedded,onReady(){}});assert.equal(host.props.queue.props.artworkContext,f.props.artworkContext);assert.equal(host.props.queue.props.detached,!embedded);f.props={...host.props.queue.props};await f.ready();assert.equal(f.thumbs()[1],"cover:Source/Album");}
 f.dispose();
});


test("cache-only fallback occupies the same two slots; cancellation prevents queued offscreen work", async()=>{
 const pending=[];const inventory=Array.from({length:20},(_,i)=>album(`a${i}`,`S/${i}`));
 const f=mount({items:inventory.map(x=>track(`${x.folder_path}/x.flac`)),inventory,cover:async()=>null,cache:async()=>{const d=defer();pending.push(d);return d.promise;}});
 await f.ready();assert.equal(pending.length,2);assert.equal(coverPaths(f.calls).length,2);
 f.props.open=false;await f.ready();pending.forEach(d=>d.resolve("old-cache"));await flush();await f.ready();assert.equal(coverPaths(f.calls).length,2);assert.ok(!f.thumbs().includes("old-cache"));f.dispose();
});

test("reused index and a fresh catalog object reject old responses while allowing new artwork",async()=>{
 const d=defer();let reads=0;const f=mount({cover:async path=>path.startsWith("Source/")&&++reads===1?d.promise:`fresh:${path}`});await f.ready();
 f.inventory=[{...a},b];f.catalog.requestRefresh();await f.ready();d.resolve("old-generation");await flush();await f.ready();assert.equal(f.thumbs()[1],"fresh:Source/Album");
 await f.replace([track("Other/Album/new.flac"),track("Other/Album/new2.flac")]);assert.deepEqual(f.thumbs(),["fresh:Other/Album","fresh:Other/Album"]);f.dispose();
});


test("closing the queue cancels its consumer without canceling another interested resolver consumer",async()=>{
 const d=defer();const f=mount({cover:async()=>d.promise});await f.ready();
 const shared=f.resolver.resolve(a,false,false,undefined,()=>true);f.props.open=false;await f.ready();d.resolve("shared-cover");assert.equal(await shared,"shared-cover");await flush();await f.ready();assert.ok(!f.thumbs().includes("shared-cover"));f.dispose();
});


test("pending image recovery is canceled by Save/indexing/unavailable and resumes after the gate clears",async()=>{
 for(const gate of ["save","index","availability"]){
  const d=defer();let first=true;const f=mount({cover:async()=>{if(first){first=false;return d.promise;}return "fresh-cover";}});await f.ready();
  if(gate==="save")f.props.artworkContext={...f.props.artworkContext,blocked:true};
  if(gate==="index")f.props.status={...f.props.status,is_updating:true};
  if(gate==="availability")f.props.available=false;
  await f.ready();d.resolve("stale-cover");await flush();await f.ready();assert.ok(!f.thumbs().includes("stale-cover"),gate);
  f.props.artworkContext={...f.props.artworkContext,blocked:false};f.props.status={...f.props.status,is_updating:false};f.props.available=true;
  await f.ready();assert.equal(f.thumbs()[1],"fresh-cover",gate);f.dispose();
 }
});

test("an already projected current cover does not trigger redundant queue artwork/catalog reads",async()=>{
 const f=mount({detached:true,items:[track("Source/Album/only.flac")]});await f.ready();assert.deepEqual(f.thumbs(),["current-cover"]);assert.deepEqual(f.calls.map(c=>c[0]),["get_queue"]);f.dispose();
});


test("filtering virtual rows uses original queue indices for current artwork and does not mutate the queue",async()=>{
 const items=Array.from({length:50},(_,i)=>({...track(`Source/Album/${i}.flac`),title:i===40?"Needle":"Other"}));
 const f=mount({items});f.props.status={...f.props.status,current_media:{kind:"local",uri:items[40].uri,queue_index:40},thumb:"current-cover"};
 await f.ready();const original=structuredClone(f.items);
 const field=nodes(f.tree,n=>n.type==="input"&&n.props.type==="search")[0];field.props.onChange({target:{value:"Needle"}});await f.ready();
 const rows=nodes(f.tree,n=>n.props?.["data-queue-index"]!==undefined);
 assert.equal(rows.length,1);assert.equal(rows[0].props["data-queue-index"],40);assert.equal(rows[0].props["aria-current"],"true");
 assert.deepEqual(f.thumbs(),["current-cover"]);assert.deepEqual(f.items,original);
 assert.equal(f.calls.filter(call=>!['get_queue','get_local_albums','get_local_covers','get_online_album_cover'].includes(call[0])).length,0);
 f.dispose();
});
