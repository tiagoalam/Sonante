import { LocalPlaybackThumbnail } from "../src/utils/localPlaybackThumbnail.ts";
import { createLocalCoverBatch } from "../src/utils/localCoverBatch.ts";
import { createLocalSourceReadQueue } from "../src/utils/localSourceReadQueue.ts";
import * as directVolumeHelpers from "../src/utils/signalPath.ts";
import * as playbackIntents from "../src/utils/playbackIntent.ts";
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import { renderToStaticMarkup } from 'react-dom/server';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import { nowPlayingWindowBounds, queueItemArtwork, queueItemIsCurrent, queueVisibleRange, QueuePreviewReader, playConfirmedQueuePosition } from '../src/utils/nowPlayingPresentation.ts';
import { analyzerOwnerMatches } from '../src/utils/nowPlayingHosting.ts';
import { routeConditions } from '../src/utils/routeConditions.ts';
import { signalPathInformation, requiresDeviceVolumeControl } from '../src/utils/signalPath.ts';
const i18n=i18next.createInstance();await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(l=>[l,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${l}.json`,import.meta.url)))}]))});
globalThis.window={localStorage:{getItem:()=>null,setItem(){}},screen:{availWidth:1920,availHeight:1040},crypto:{randomUUID:()=> 'synthetic-session'},setTimeout,clearTimeout,addEventListener(){},removeEventListener(){}};
const dependencies={'../utils/localCoverBatch':{createLocalCoverBatch},'../utils/localSourceReadQueue':{createLocalSourceReadQueue,localSourceReadQueue:createLocalSourceReadQueue()},'./config':{ensureDirectOutputChoice:async()=>{}},'../utils/playbackIntent':{...playbackIntents,beginPlaybackIntent:()=>({current:()=>true,confirmCurrent:async()=>true,token:Promise.resolve(null),cancel(){}}),configurePlaybackIntents(){}},'../utils/useLocalQueueArtwork':{useLocalQueueArtwork:()=>item=>item},'react':{...React,default:React},'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../utils/nowPlayingHosting':{analyzerOwnerMatches},'../utils/signalPath':{...directVolumeHelpers,signalPathInformation,requiresDeviceVolumeControl},'../utils/routeConditions':{routeConditions},'../utils/nowPlayingPresentation':{nowPlayingWindowBounds,queueItemArtwork,queueItemIsCurrent,queueVisibleRange,QueuePreviewReader,playConfirmedQueuePosition},'./nowPlaying.css':{},'../utils/frontendDiagnostics':{logError(){}}};
const compile=(file,extra={})=>{const js=ts.transpileModule(readFileSync(new URL(`../src/${file}`,import.meta.url),'utf8'),{fileName:file,compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const ex={};new Function('require','exports',js)(name=>{assert.ok(name in extra||name in dependencies,name);return extra[name]??dependencies[name];},ex);return ex;};
dependencies['./PlexImage']={PlexImage:({alt,className})=>React.createElement('div',{'data-plex-artwork':true,'aria-label':alt,className})};
dependencies['./TrackArtwork']=compile('components/TrackArtwork.tsx');
dependencies['./SignalPath']=compile('components/SignalPath.tsx');
dependencies['./PlaybackOptionsControls']=compile('components/PlaybackOptionsControls.tsx',{'../services/audio':{audioService:{changePlaybackOption:async()=>{throw new Error('unexpected option request');}}}});
const graph=name=>({[name]:props=>React.createElement('svg',{'aria-label':props.label??name,role:'img',viewBox:'0 0 480 120',preserveAspectRatio:'none',style:{width:'100%',height:name==='VuHorizontal'?28:'100%'}},Array.from({length:40},(_,i)=>React.createElement('rect',{key:i,x:i*12,y:120-(25+(i*17)%85),width:8,height:25+(i*17)%85,rx:1,fill:'#E5A00D',opacity:.6})))});
for(const name of ['AnalogVu','SpectrumNeon','SpectrumSegmented','VuHorizontal']) dependencies[`./${name}`]=graph(name);
dependencies['./visualizer/VisualizerPanel']=compile('components/visualizer/VisualizerPanel.tsx');
dependencies['../utils/playerNavigation']={currentMediaKey:media=>JSON.stringify(media)};
dependencies['./SourceAudioBadges']={SourceAudioBadges:()=>null};
const {NowPlayingPresentation:View}=compile('components/NowPlayingPresentation.tsx');
const snapshot={route:{mode:'shared',endpoint:'default'},health:{state:'available'},playback:{state:'play',elapsed:57,duration:185,audio_format:'96000:24:2',current_media:{kind:'local',uri:'synthetic.flac',queue_index:1},title:'A synthetic song with a deliberately long title that remains above the playback controls',artist:'Synthetic artist',album:'Synthetic album',volume:{value:43,muted:false,writable:true,available:true,backend:'pipewire'},is_updating:false}};
const props={snapshot,levels:{leftRms:.2,rightRms:.3,leftPeak:.4,rightPeak:.5,spectrum:Array(48).fill(.2)},fullscreen:false,queueOpen:false,queue:null,analyzerAvailable:true,analyzerInactiveLabel:'Unavailable',controlFailed:false,isSeeking:false,seekValue:57};
for(const name of ['onSeekStart','onSeekChange','onSeekCommit','onPrevious','onTogglePlay','onNext','onVolume','onMute','onQueue','onFullscreen','onClose'])props[name]=()=>{};
const nodes=(tree,pred)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,pred)):!tree||typeof tree!=='object'?[]:[...(pred(tree)?[tree]:[]),...nodes(tree.props?.children,pred)];
test('presentation renders long metadata, all controls and confirmed horizontal signal in both languages/window modes',async()=>{
 for(const lng of ['en','pt-BR']){await i18n.changeLanguage(lng);for(const fullscreen of [false,true]){
  const html=renderToStaticMarkup(React.createElement(View,{...props,fullscreen,queueOpen:true,queue:React.createElement('aside',{id:'now-playing-queue'},'Synthetic queue')}));
  assert.match(html,/Synthetic artist/);assert.match(html,/Synthetic album/);assert.match(html,/aria-haspopup="dialog"/);assert.match(html,/96 kHz/);assert.match(html,/24-bit/);assert.match(html,/np-signal-stages/);assert.doesNotMatch(html,/data-route-conditions|data-alsa-observation/);assert.match(html,/aria-controls="now-playing-queue"/);assert.match(html,/aria-expanded="true"/);assert.doesNotMatch(html,/nowPlaying\.|player\.seek/i);assert.match(html,/data-signal-summary/);assert.doesNotMatch(html,/data-bitperfect/);
  for(const label of [i18n.t('player.previous'),i18n.t('player.pause'),i18n.t('player.next'),i18n.t('nowPlaying.seek'),i18n.t('signalPath.volume')])assert.ok(html.includes(label),label);
 }}await i18n.changeLanguage('en');
});
test('queue/fullscreen/close are distinct callbacks; only playback controls submit playback actions',()=>{
 const calls=[];const p={...props};for(const name of Object.keys(p).filter(n=>n.startsWith('on')))p[name]=(...args)=>calls.push([name,...args]);
 const tree=View(p);const byLabel=label=>nodes(tree,n=>n.type==='button'&&n.props['aria-label']===label)[0];
 byLabel(i18n.t('nowPlaying.showQueue')).props.onClick();byLabel(i18n.t('nowPlaying.enterFullscreen')).props.onClick();byLabel(i18n.t('nowPlaying.close')).props.onClick();assert.deepEqual(calls,[['onQueue'],['onFullscreen'],['onClose']]);
 byLabel(i18n.t('player.pause')).props.onClick();byLabel(i18n.t('player.next')).props.onClick();byLabel(i18n.t('player.previous')).props.onClick();assert.deepEqual(calls.slice(3),[['onTogglePlay'],['onNext'],['onPrevious']]);
 const seek=nodes(tree,n=>n.type==='input'&&n.props['aria-label']===i18n.t('nowPlaying.seek'))[0];seek.props.onChange({target:{value:'60'}});seek.props.onKeyUp({key:'ArrowRight',currentTarget:{value:'60'}});assert.deepEqual(calls.slice(-3),[['onSeekStart'],['onSeekChange',60],['onSeekCommit',60]]);
});
test('unavailable playback disables seek, transport and volume and shows neutral failure',()=>{
 const p={...props,snapshot:{...snapshot,health:{state:'unavailable',reason:'socket_unavailable'}},controlFailed:true};const tree=View(p);
 for(const n of nodes(tree,n=>n.type==='input'||n.type==='button'&&[i18n.t('player.previous'),i18n.t('player.pause'),i18n.t('player.next'),i18n.t('player.mute')].includes(n.props['aria-label'])))assert.equal(n.props.disabled,true);
 const html=renderToStaticMarkup(React.createElement(View,p));assert.match(html,/could not be completed|failed|unavailable/i);assert.doesNotMatch(html,/96 kHz/);
});
test('virtual queue shares artwork/actions and retains duplicate positions/current paused identity',async()=>{
 const local={title:'Duplicated song',artist:'Artist',media_locator:{kind:'local',uri:'same.flac'}};
 let stateIndex=0;const calls=[],states=[{items:[local,local],phase:'ready'},{top:0,height:480},false,false];
 const hooks={...React,default:React,useState:initial=>[states[stateIndex++]??initial,()=>{}],useEffect(){},useRef:initial=>({current:initial})};
 const {NowPlayingQueue}=compile('components/NowPlayingQueue.tsx',{'react':hooks,'../services/audio':{audioService:{getQueue:async()=>{calls.push('read');return[local,local];},playQueueIndex:async index=>calls.push(['play',index]),clearQueue:async()=>calls.push('clear')}}});
 const tree=NowPlayingQueue({open:true,available:true,status:{...snapshot.playback,state:'pause',current_media:{kind:'local',uri:'same.flac',queue_index:1}}});
 const rows=nodes(tree,n=>n.type==='li');assert.equal(rows.length,2);assert.equal(rows[0].props['aria-current'],undefined);assert.equal(rows[1].props['aria-current'],'true');assert.equal(rows[1].props['aria-posinset'],2);assert.deepEqual(calls,[]);
 nodes(rows[1],n=>n.type==='button')[0].props.onClick();nodes(rows[1],n=>n.type==='button')[0].props.onClick();for(let i=0;i<12;i++)await Promise.resolve();assert.deepEqual(calls,['read',['play',1]]);
 nodes(tree,n=>n.type==='button'&&n.props['aria-label']===i18n.t('queue.clear'))[0].props.onClick();for(let i=0;i<12;i++)await Promise.resolve();assert.equal(calls.at(-1),'clear');
});
const flush=async()=>{for(let i=0;i<20;i++)await Promise.resolve();};
const owner={host:'main',sessionId:'synthetic-session',generation:1};
test('seek gesture submits once through the real service, in both hosts, with the slider seconds unchanged',async()=>{
 for(const embedded of [true,false]){
  const calls=[];
  const {audioService}=compile('services/audio.ts',{'@tauri-apps/api/core':{invoke:async(command,args)=>calls.push([command,args])},'../utils/frontendDiagnostics':{diagnosticInvoke:(invoke,command,args)=>invoke(command,args)},'../types/audio':{},'../types/local':{},'../utils/localArtwork':{}});
  const hooks={...React,default:React,useEffect(){},useState:value=>[value,()=>{}],useRef:value=>({current:value})};
  const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService},'../services/nowPlayingWindow':{}});
  const host=NowPlayingHost({snapshot,owner:{...owner,host:embedded?'main':'now-playing'},embedded,onReady(){}});
  const tree=View(host.props);const seek=nodes(tree,node=>node.type==='input'&&node.props['aria-label']===i18n.t('nowPlaying.seek'))[0];
  assert.equal(seek.props.max,snapshot.playback.duration);
  seek.props.onPointerDown();seek.props.onChange({target:{value:'60'}});seek.props.onChange({target:{value:'62.5'}});assert.deepEqual(calls,[]);
  seek.props.onPointerUp({currentTarget:{value:'62.5'}});await flush();assert.deepEqual(calls,[['seek_playback',{seconds:62.5}]]);
  seek.props.onKeyUp({key:'Tab',currentTarget:{value:'62.5'}});await flush();assert.equal(calls.length,1);
  seek.props.onKeyUp({key:'ArrowRight',currentTarget:{value:'63'}});await flush();assert.deepEqual(calls.at(-1),['seek_playback',{seconds:63}]);
 }
});

test('seek handlers keep unavailable or transferring hosts from submitting transport commands',async()=>{
 const calls=[];const hooks={...React,default:React,useEffect(){},useState:value=>[value,()=>{}],useRef:value=>({current:value})};
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService:{seek:async()=>calls.push('seek')}},'../services/nowPlayingWindow':{}});
 for(const extra of [{busy:true},{owner:null},{snapshot:{...snapshot,health:{state:'unavailable',reason:'socket_unavailable'}}}]){
  const host=NowPlayingHost({snapshot,owner,embedded:true,onReady(){},...extra});host.props.onSeekCommit(62.5);await flush();
 }
 assert.deepEqual(calls,[]);
});

// Explicit hook renders model local state and playback updates. Native range
// event ordering/WebKitGTK and actual PCM consumption are not simulated here.
function seekControlHarness(kind,invoke){
 const slots=[],effects=[];let cursor=0,effectIndex=0;
 const hooks={...React,default:React,
  useState:initial=>{const i=cursor++;slots[i]??={value:initial};return[slots[i].value,value=>{slots[i].value=value;}];},
  useRef:initial=>{const i=cursor++;return slots[i]??={current:initial};},
  useEffect:(fn,deps)=>{const i=cursor++;const index=effectIndex++;const previous=slots[i];
   if(!previous||deps.some((value,n)=>!Object.is(value,previous[n]))){slots[i]=deps;if(index<2)effects.push(fn);}
  },
 };
 const {audioService}=compile('services/audio.ts',{'@tauri-apps/api/core':{invoke},'../utils/frontendDiagnostics':{diagnosticInvoke:(native,command,args)=>native(command,args)},'../types/audio':{},'../types/local':{},'../utils/localArtwork':{}});
 const component=kind==='bar'
  ?compile('components/PlayerBar.tsx',{'react':hooks,'../services/audio':{audioService}}).PlayerBar
  :compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService},'../services/nowPlayingWindow':{}}).NowPlayingHost;
 return playback=>{
  cursor=0;effectIndex=0;effects.length=0;
  const rendered=kind==='bar'?component({status:playback,health:snapshot.health,route:snapshot.route,onToggleQueue(){},isQueueOpen:false})
   :component({snapshot:{...snapshot,playback},owner:{...owner,host:kind==='embedded'?'main':'now-playing'},embedded:kind==='embedded',onReady(){}});
  for(const effect of effects)effect();
  const tree=kind==='bar'?rendered:View(rendered.props);
  return nodes(tree,n=>n.type==='input'&&n.props.type==='range')[0];
 };
}

test('PlayerBar and both Now Playing controls send identical single seeks across drag, playback updates and delayed ACK',async()=>{
 for(const kind of ['bar','embedded','detached']){
  const calls=[];let acknowledge;
  const render=seekControlHarness(kind,(command,args)=>{calls.push([command,args]);return new Promise(resolve=>acknowledge=resolve);});
  let playback={...snapshot.playback,duration:445.215};
  for(const target of [260,312,342]){
   let seek=render(playback);
   (kind==='bar'?seek.props.onMouseDown:seek.props.onPointerDown)();seek=render(playback);
   seek.props.onChange({target:{value:String(target)}});seek=render(playback);
   playback={...playback,elapsed:playback.elapsed+1};seek=render(playback);
   assert.equal(seek.props.value,target,'normal telemetry does not overwrite the drag');
   const count=calls.length;
   const release={target:{value:String(target)},currentTarget:{value:String(target)}};
   (kind==='bar'?seek.props.onMouseUp:seek.props.onPointerUp)(release);
   await flush();assert.deepEqual(calls.slice(count),[['seek_playback',{seconds:target}]]);
   // The other compatibility event cannot submit another request.
   assert.equal(kind==='bar'?seek.props.onPointerUp:seek.props.onMouseUp,undefined);
   playback={...playback,elapsed:target+.073};render(playback);
   playback={...playback,elapsed:target+.157};render(playback);
   acknowledge();await flush();render(playback);
   for(const elapsed of [target+.482,target+.482]){
    playback={...playback,elapsed};seek=render(playback);assert.equal(seek.props.value,elapsed);
   }
   assert.equal(calls.length,count+1,'ACK and frozen telemetry issue no implicit transport commands');
  }
  assert.deepEqual(calls,[260,312,342].map(seconds=>['seek_playback',{seconds}]));
 }
});

test('all three seek controls preserve paused/stopped state and never compensate a rejected IPC with transport',async()=>{
 for(const kind of ['bar','embedded','detached'])for(const state of ['pause','stop']){
  const calls=[];const render=seekControlHarness(kind,async(command,args)=>{calls.push([command,args]);throw new Error('synthetic seek rejection');});
  const playback={...snapshot.playback,state};let seek=render(playback);
  (kind==='bar'?seek.props.onMouseDown:seek.props.onPointerDown)();seek=render(playback);
  seek.props.onChange({target:{value:'62.5'}});seek=render(playback);
  const release={target:{value:'62.5'},currentTarget:{value:'62.5'}};
  (kind==='bar'?seek.props.onMouseUp:seek.props.onPointerUp)(release);
  await flush();render(playback);
  assert.deepEqual(calls,[['seek_playback',{seconds:62.5}]]);assert.equal(playback.state,state);
 }
});

test('shared host filters old generations, stops on cleanup and ignores disposed events',async()=>{
 const effects=[],listeners=new Map(),calls=[],setters=[];let lease;
 const hooks={...React,default:React,useEffect:fn=>effects.push(fn),useState:v=>[v,value=>setters.push(value)],useRef:v=>({current:v})};
 const event={listen:async(name,fn)=>{listeners.set(name,fn);return ()=>{listeners.delete(name);calls.push('unlisten');};}};
 const services={AUDIO_LEVEL_EVENT:'level',ANALYZER_STATUS_EVENT:'analyzer',startAudioAnalyzer:async (received,leaseId)=>{lease=leaseId;assert.equal(received,owner);calls.push('start');},stopAudioAnalyzer:async received=>{assert.equal(received,owner);calls.push('stop');}};
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':event,'@tauri-apps/api/window':{getCurrentWindow:()=>({isFullscreen:async()=>false})},'../services/audio':{audioService:{}},'../services/nowPlayingWindow':services});
 NowPlayingHost({snapshot,owner,embedded:true,onReady:()=>calls.push('ready')});const cleanup=effects[2]();await flush();assert.deepEqual(calls,['start','ready']);
 const old=listeners.get('level');const before=setters.length;old({payload:{owner:{...owner,generation:0,leaseId:lease},data:props.levels}});assert.equal(setters.length,before);old({payload:{owner:{...owner,leaseId:'obsolete-cycle'},data:props.levels}});assert.equal(setters.length,before);old({payload:{owner:{...owner,leaseId:lease},data:props.levels}});assert.equal(setters.at(-1),props.levels);
 cleanup();await flush();assert.equal(calls.at(-1),'stop');const after=setters.length;old({payload:{owner:{...owner,leaseId:lease},data:props.levels}});assert.equal(setters.length,after);assert.equal(listeners.size,0);
});
test('closing before listener readiness never starts a reader and removes late listeners',async()=>{
 const effects=[],calls=[];let resolve;const pending=new Promise(r=>resolve=r);const hooks={...React,default:React,useEffect:fn=>effects.push(fn),useState:v=>[v,()=>{}],useRef:v=>({current:v})};
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{listen:async()=>{await pending;return()=>calls.push('unlisten');}},'@tauri-apps/api/window':{},'../services/audio':{audioService:{}},'../services/nowPlayingWindow':{startAudioAnalyzer:async()=>calls.push('start'),stopAudioAnalyzer:async()=>calls.push('stop')}});
 NowPlayingHost({snapshot,owner,embedded:true,onReady(){}});const cleanup=effects[2]();cleanup();resolve();await flush();assert.deepEqual(calls,['unlisten','unlisten']);
});
test('window service reuses one window, waits for nonce readiness and sizes the main monitor',async()=>{
 let existing={setFocus:async()=>calls.push('focus'),once:async()=>()=>{}};let options;const calls=[],listeners=new Map();
 class WebviewWindow{static async getByLabel(){return existing;}constructor(label,o){assert.equal(label,'now-playing');options=o;existing=this;calls.push('create');}async once(name,fn){if(name==='tauri://created')fn();return()=>{};}}
 const area={position:{x:-1920,y:0},size:{width:1920,height:1040}};
 const ex=compile('services/nowPlayingWindow.ts',{'@tauri-apps/api/event':{listen:async(name,fn)=>{listeners.set(name,fn);return()=>listeners.delete(name);},emitTo:async(label,event,data)=>{listeners.get('now-playing://host-reply')?.({payload:{requestId:data.requestId,runtime:'synthetic-runtime',ok:true}});}},'@tauri-apps/api/core':{invoke:async()=>{}},'@tauri-apps/api/webviewWindow':{WebviewWindow},'@tauri-apps/api/window':{currentMonitor:async()=>({workArea:area,scaleFactor:1.25})}});
 assert.equal(await ex.openNowPlayingWindow('Synthetic'),'synthetic-runtime');assert.deepEqual(calls,['focus']);existing=null;
 await Promise.all([ex.openNowPlayingWindow('Synthetic'),ex.openNowPlayingWindow('Synthetic')]);assert.deepEqual(calls,['focus','create']);assert.equal(options.fullscreen,false);assert.deepEqual(Object.fromEntries(Object.keys(nowPlayingWindowBounds(area,1.25)).map(k=>[k,options[k]])),nowPlayingWindowBounds(area,1.25));assert.equal(listeners.size,0);
});
test('all five visualizer modes and circular selection remain available and do not request audio',()=>{
 const modes=['combo','spectrum-neon','spectrum-segmented','vu-amber','vu-blue'];
 for(let index=0;index<modes.length;index++){
  const selected=[];const hooks={...React,default:React,useState:()=>[modes[index],v=>selected.push(v)],useEffect(){}};
  const {VisualizerPanel}=compile('components/visualizer/VisualizerPanel.tsx',{'react':hooks});
  const tree=VisualizerPanel({levels:props.levels,active:true,fullscreen:true});
  assert.equal(tree.props['aria-label'],i18n.t(`nowPlaying.visualizer.modes.${modes[index]}`));
  const buttons=nodes(tree,n=>n.type==='button');buttons[0].props.onClick();buttons[1].props.onClick();
  assert.deepEqual(selected,[modes[(index+4)%5],modes[(index+1)%5]]);
 }
});
test('artwork forwards Plex references unchanged and local errors retain the fallback',()=>{
 const reference={server_id:'synthetic',path:'/library/metadata/1/thumb',width:100,height:100};
 const {TrackArtwork}=dependencies['./TrackArtwork'];
 const {TrackArtwork:Stubbed}=compile('components/TrackArtwork.tsx',{'react':{...React,default:React,useState:()=>[null,()=>{}]}});
 const tree=Stubbed({track:{plex_image:reference,thumb:'ignored'},alt:'Album'});
 const node=nodes(tree,n=>n.type===dependencies['./PlexImage'].PlexImage)[0];assert.equal(node.props.image,reference);assert.ok(node.props.fallback);
 let failed;const {TrackArtwork:Local}=compile('components/TrackArtwork.tsx',{'react':{...React,default:React,useState:()=>[null,v=>failed=v]}});
 const image=nodes(Local({track:{thumb:'synthetic-cover'},alt:'Album'}),n=>n.type==='img')[0];image.props.onError();assert.equal(failed,'synthetic-cover');assert.equal(image.props.loading,'lazy');
 const {TrackArtwork:Fallback}=compile('components/TrackArtwork.tsx',{'react':{...React,default:React,useState:()=>['synthetic-cover',()=>{}]}});
 assert.equal(nodes(Fallback({track:{thumb:'synthetic-cover'}}),n=>n.type==='img').length,0);
});
// Optional artifact uses the actual presentation/component structure with synthetic
// metadata and canvas placeholders. It is not a browser or WebKitGTK validation.
if(process.env.SONANTE_SYNTHETIC_PREVIEW){
 await i18n.changeLanguage('pt-BR');
 const cover='data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 600 600"><defs><linearGradient id="c" x2="1" y2="1"><stop stop-color="#31474B"/><stop offset="1" stop-color="#76513A"/></linearGradient></defs><rect width="600" height="600" fill="url(#c)"/><circle cx="300" cy="270" r="170" fill="none" stroke="#E5A00D" stroke-width="3"/><circle cx="300" cy="270" r="80" fill="#101010"/><text x="300" y="510" text-anchor="middle" fill="white" font-family="sans-serif" font-size="32">SONANTE · SINTÉTICO</text></svg>');
 const previewProps={...props,snapshot:{...snapshot,playback:{...snapshot.playback,thumb:cover}}};
 const css=readFileSync(process.env.SONANTE_PREVIEW_CSS,'utf8');
 const frames=[[1280,720,false,false],[1280,720,true,false],[1920,1080,true,false],[1280,800,false,true],[1280,800,true,true],[640,800,true,true]].map(([w,h,open,embedded])=>{
  const items=Array.from({length:8},(_,i)=>({title:`Faixa sintética ${i+1}`,artist:'Artista sintético',thumb:cover,media_locator:{kind:'local',uri:i===1?'synthetic.flac':`synthetic-${i}.flac`}}));
  let queueState=0;
  const PreviewQueue=compile('components/NowPlayingQueue.tsx',{'react':{...React,default:React,useEffect(){},useState:initial=>[[{items,phase:'ready'},{top:0,height:600},false,false][queueState++]??initial,()=>{}],useRef:value=>({current:value})},'../services/audio':{audioService:{getQueue:async()=>items}}}).NowPlayingQueue;
  const queue=React.createElement(PreviewQueue,{open,available:true,status:snapshot.playback});
  const content=renderToStaticMarkup(React.createElement(View,{...previewProps,embedded,fullscreen:w===1920,queueOpen:open,queue}));
  const html=embedded ? `<div style="height:100dvh;display:flex"><aside style="width:224px;flex:none;background:#181818;padding:24px">SONANTE<br><br>Biblioteca sintética</aside><div style="flex:1;min-width:0;height:100%">${content}</div></div>` : content;
  return `<h2 style="margin:24px">${w} × ${h} · ${embedded?'incorporado':'destacado'} · fila ${open?'aberta':'fechada'}</h2><iframe title="Preview ${w} ${h}" style="width:${w}px;height:${h}px;border:1px solid #555" srcdoc="${(`<!doctype html><html><head><meta charset='utf-8'><style>${css}</style></head><body>${html}</body></html>`).replaceAll('&','&amp;').replaceAll('"','&quot;')}"></iframe>`;
 });writeFileSync(process.env.SONANTE_SYNTHETIC_PREVIEW,`<!doctype html><meta charset="utf-8"><title>Sonante · preview sintético</title><body style="background:#151515;color:white;font-family:system-ui">${frames.join('')}</body>`);
}

test('detached requests require runtime/transition and reject delayed activation after suspension',async()=>{
 const effects=[],slots=[],handlers=new Map(),replies=[];let cursor=0;
 const hooks={...React,default:React,useEffect:fn=>effects.push(fn),useRef:value=>{const i=cursor++;return slots[i]??={current:value};},useState:value=>{const i=cursor++;if(!(i in slots))slots[i]=value;return[slots[i],next=>slots[i]=next];}};
 const {NowPlayingWindow}=compile('components/NowPlayingWindow.tsx',{'react':hooks,'./NowPlayingHost':{NowPlayingHost:()=>null},'@tauri-apps/api/event':{listen:async(name,fn)=>{handlers.set(name,fn);return()=>handlers.delete(name);},emitTo:async(target,event,payload)=>replies.push([event,payload])},'../services/nowPlayingWindow':{HOST_REQUEST_EVENT:'request',HOST_REPLY_EVENT:'reply',NAVIGATE_EVENT:'navigate',NOW_PLAYING_READY_EVENT:'ready',NOW_PLAYING_SNAPSHOT_EVENT:'snapshot',REINCORPORATE_EVENT:'reincorporate'}});
 const render=()=>{cursor=0;return NowPlayingWindow();};render();const cleanup=effects[0]();await flush();
 handlers.get('snapshot')({payload:snapshot});
 const request=data=>handlers.get('request')({payload:data});const target={...owner,host:'now-playing'};
 request({action:'activate',requestId:'a',runtime:'wrong',owner:target,transition:1});assert.equal(replies.at(-1)[1].ok,false);
 request({action:'activate',requestId:'b',runtime:'synthetic-session',owner:target,transition:2});const active=render();assert.equal(active.props.owner,target);active.props.onReady(target);await flush();assert.equal(replies.at(-1)[1].requestId,'b');assert.equal(replies.at(-1)[1].ok,true);
 request({action:'suspend',requestId:'c',runtime:'synthetic-session',transition:3});assert.equal(render().props.owner,null);
 active.props.onReady(target);request({action:'activate',requestId:'d',runtime:'synthetic-session',owner:target,transition:2});assert.equal(render().props.owner,null);assert.equal(replies.at(-1)[1].ok,false);cleanup();assert.equal(handlers.size,0);
});
test('App uses one visible primary control set, keeps previous page mounted and gates the shared drawer',()=>{
 const source=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let bar,embedded,page;
 const hasAttr=(node,name)=>node.attributes.properties.some(p=>ts.isJsxAttribute(p)&&p.name.getText(ast)===name);
 const walk=node=>{if(ts.isJsxElement(node)){if(hasAttr(node.openingElement,'data-now-playing-player-bar'))bar=node;if(hasAttr(node.openingElement,'data-now-playing-embedded'))embedded=node;if(hasAttr(node.openingElement,'data-now-playing-previous-page'))page=node;}ts.forEachChild(node,walk);};walk(ast);assert.ok(bar&&embedded&&page);
 const hidden=bar.openingElement.attributes.properties.find(p=>p.name?.getText(ast)==='hidden').initializer.expression.getText(ast);assert.equal(hidden,'nowPlayingEmbedded');
 assert.ok(page.children.some(n=>ts.isJsxElement(n)||ts.isJsxExpression(n)));assert.match(page.openingElement.getText(ast),/inert=\{nowPlayingEmbedded/);
 assert.equal(embedded.parent.left.getText(ast),'nowPlayingEmbedded');
 for(const isEmbedded of [false,true]){const barVisible=!new Function('nowPlayingEmbedded',`return ${hidden}`)(isEmbedded);const npVisible=isEmbedded;assert.equal(Number(barVisible)+Number(npVisible),1);}
 assert.match(source,/isOpen=\{showQueue && !nowPlayingEmbedded\}/);assert.match(source,/onOpenNowPlaying=\{showNowPlaying\}/);
 const drawer=readFileSync(new URL('../src/components/QueueDrawer.tsx',import.meta.url),'utf8');assert.match(drawer,/<NowPlayingQueue/);assert.doesNotMatch(drawer,/audioService|fetchQueue|playQueueIndex|clearQueue/);
});
test('embedded presentation has detach/back and album/artist navigation, with no fullscreen control',()=>{
 const calls=[];const tree=View({...props,embedded:true,onDetach:()=>calls.push('detach'),onNavigate:view=>calls.push(view),onClose:()=>calls.push('back')});
 const buttons=nodes(tree,n=>n.type==='button');assert.equal(buttons.some(n=>n.props['aria-label']===i18n.t('nowPlaying.enterFullscreen')),false);
 buttons.find(n=>n.props['aria-label']===i18n.t('nowPlaying.detach')).props.onClick();buttons.find(n=>n.props['aria-label']===i18n.t('nowPlaying.collapse')).props.onClick();
 buttons.find(n=>n.props.children==='Synthetic artist').props.onClick();buttons.find(n=>n.props.children==='Synthetic album').props.onClick();assert.deepEqual(calls,['detach','back','artist','album']);
});

test('analyzer start/stop is serialized across StrictMode replay and carries generation to IPC',async()=>{
 const calls=[];let release;const first=new Promise(r=>release=r);
 const ex=compile('services/nowPlayingWindow.ts',{'@tauri-apps/api/event':{},'@tauri-apps/api/core':{invoke:async(command,args)=>{calls.push([command,args.generation,args.sessionId,args.leaseId]);if(calls.length===1)await first;}},'@tauri-apps/api/webviewWindow':{},'@tauri-apps/api/window':{}});
 const a=ex.startAudioAnalyzer(owner,"first");const stop=ex.stopAudioAnalyzer(owner,"first");const b=ex.startAudioAnalyzer({...owner,generation:2},"second");await flush();assert.equal(calls.length,1);release();await Promise.all([a,stop,b]);assert.deepEqual(calls.map(c=>c.slice(0,2)),[['start_audio_analyzer',1],['stop_audio_analyzer',1],['start_audio_analyzer',2]]);assert.deepEqual(calls.map(c=>c[3]),['first','first','second']);
});
test('late readiness listener registration after timeout is immediately cleaned, without polling',async()=>{
 const saved={setTimeout:globalThis.setTimeout,clearTimeout:globalThis.clearTimeout,setInterval:globalThis.setInterval};let resolveRegistration,cleaned=0;
 try{
  globalThis.setTimeout=fn=>{queueMicrotask(fn);return 1;};globalThis.clearTimeout=()=>{};globalThis.setInterval=()=>assert.fail('late poll');
  const ex=compile('services/nowPlayingWindow.ts',{'@tauri-apps/api/event':{listen:()=>new Promise(resolve=>resolveRegistration=resolve)},'@tauri-apps/api/core':{},'@tauri-apps/api/webviewWindow':{},'@tauri-apps/api/window':{}});
  await assert.rejects(ex.exchangeDetachedHost('ready'));resolveRegistration(()=>cleaned++);await flush();assert.equal(cleaned,1);
 }finally{Object.assign(globalThis,saved);}
});
test('queue clear failure stays visible and preserves the displayed items',async()=>{
 const local={title:'Preserved',artist:'Artist',media_locator:{kind:'local',uri:'fixture.flac'}};let cursor=0;const slots=[{items:[local],phase:'ready'},{top:0,height:480},false,false],refs=[];
 const hooks={...React,default:React,useState:initial=>{const i=cursor++;if(!(i in slots))slots[i]=initial;return[slots[i],v=>slots[i]=v];},useEffect(){},useRef:initial=>{const i=cursor++;return refs[i]??={current:initial};}};
 const {NowPlayingQueue}=compile('components/NowPlayingQueue.tsx',{'react':hooks,'../services/audio':{audioService:{clearQueue:async()=>{throw Error('private backend message');}}}});
 const render=()=>{cursor=0;return NowPlayingQueue({open:true,available:true,status:snapshot.playback});};let tree=render();nodes(tree,n=>n.type==='button'&&n.props['aria-label']===i18n.t('queue.clear'))[0].props.onClick();await flush();tree=render();assert.equal(slots[0].items[0],local);const alerts=nodes(tree,n=>n.props?.role==='alert');assert.equal(alerts.length,1);assert.equal(alerts[0].props.children,i18n.t('nowPlaying.queueActionFailed'));
});

test('embedded and detached hosting actions resolve in pt-BR/en without changing confirmed signal metadata',async()=>{
 for(const lng of ['pt-BR','en']){
  await i18n.changeLanguage(lng);
  for(const embedded of [false,true]){
   const html=renderToStaticMarkup(React.createElement(View,{...props,embedded,hostingError:true,controlsDisabled:true}));
   assert.ok(html.includes(i18n.t(embedded?'nowPlaying.detach':'nowPlaying.reincorporate')));
   assert.ok(html.includes(i18n.t(embedded?'nowPlaying.collapse':'nowPlaying.close')));
   assert.match(html,/96 kHz/);assert.match(html,/24-bit/);assert.match(html,/np-signal-stages/);assert.doesNotMatch(html,/data-route-conditions|data-alsa-observation/);assert.doesNotMatch(html,/nowPlaying\./i);assert.match(html,/data-signal-summary/);assert.doesNotMatch(html,/data-bitperfect/);
  }
 }await i18n.changeLanguage('en');
});


test('shared option controls render MPD snapshots, reject overlapping clicks and expose failure without optimistic mode',async()=>{
 const slots=[];let cursor=0;const calls=[];let resolve,reject;
 const hooks={...React,default:React,useState:initial=>{const i=cursor++;slots[i]??={value:initial};return[slots[i].value,v=>{slots[i].value=v;}];},useRef:initial=>{const i=cursor++;return slots[i]??={current:initial};}};
 const {PlaybackOptionsControls,repeatMode}=compile('components/PlaybackOptionsControls.tsx',{'react':hooks,'../services/audio':{audioService:{changePlaybackOption:(action,expected)=>{calls.push({action,expected});return new Promise((a,b)=>{resolve=a;reject=b;});}}}});
 const off={repeat:false,single:'off',random:false};const all={...off,repeat:true};const one={...all,single:'on'};
 assert.equal(repeatMode(off),'off');assert.equal(repeatMode(all),'all');assert.equal(repeatMode(one),'one');assert.equal(repeatMode({...off,single:'oneshot'}),'custom');
 const render=options=>{cursor=0;return PlaybackOptionsControls({options,disabled:false});};
 let tree=render(off);let buttons=nodes(tree,n=>n.type==='button');
 buttons[0].props.onClick();buttons[0].props.onClick();buttons[1].props.onClick();assert.equal(calls.length,1);
 tree=render(off);assert.ok(nodes(tree,n=>n.type==='button').every(n=>n.props.disabled));assert.equal(nodes(tree,n=>n.type==='button')[0].props['aria-pressed'],false);
 resolve(all);await flush();tree=render(off);assert.equal(nodes(tree,n=>n.type==='button')[0].props['aria-pressed'],false,'ACK does not paint requested mode over an older status');
 tree=render(all);assert.equal(nodes(tree,n=>n.type==='button')[0].props['aria-pressed'],true);
 nodes(tree,n=>n.type==='button')[0].props.onClick();reject(new Error('partial ACK'));await flush();tree=render({...off,single:'on'});assert.equal(nodes(tree,n=>n.props?.role==='alert').length,1);assert.match(nodes(tree,n=>n.type==='button')[0].props.title,/MPD/);
 for(const options of [one,{...one,random:true},off]){
  tree=render(options);buttons=nodes(tree,n=>n.type==='button');assert.equal(buttons[1].props['aria-pressed'],options.random);
  const html=renderToStaticMarkup(tree);assert.equal(html.includes('>1</span>'),options===one || options.single==='on'&&options.repeat);assert.doesNotMatch(html,/player\.repeat|player\.random/);
 }
 tree=render(null);assert.ok(nodes(tree,n=>n.type==='button').every(n=>n.props.disabled));
 assert.deepEqual(calls.map(x=>x.action),['cycle_repeat','cycle_repeat']);
});

test('all three players forward the same confirmed options; both languages resolve labels and indicator',async()=>{
 const options={repeat:true,single:'on',random:true};
 const hooks={...React,default:React,useState:initial=>[initial,()=>{}],useRef:initial=>({current:initial}),useEffect(){}};
 const {PlayerBar}=compile('components/PlayerBar.tsx',{'react':hooks,'../services/audio':{audioService:{}}});
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService:{}},'../services/nowPlayingWindow':{}});
 const Controls=dependencies['./PlaybackOptionsControls'].PlaybackOptionsControls;
 const current={...snapshot.playback,options};
 const bar=PlayerBar({status:current,health:snapshot.health,route:snapshot.route,onToggleQueue(){},isQueueOpen:false});
 assert.equal(nodes(bar,n=>n.type===Controls)[0].props.options,options);
 for(const embedded of [true,false]){
  const host=NowPlayingHost({snapshot:{...snapshot,playback:current},owner,embedded,onReady(){}});
  assert.equal(host.props.optionsControls.type,Controls);assert.equal(host.props.optionsControls.props.options,options);
  assert.equal(host.props.queueOpen,true,'default-visible queue is retained');
 }
 for(const language of ['en','pt-BR']){
  await i18n.changeLanguage(language);
  const html=renderToStaticMarkup(React.createElement(Controls,{options}));
  assert.match(html,/aria-pressed="true"/);assert.match(html,/>1<\/span>/);assert.doesNotMatch(html,/player\.repeat|player\.random/);
 }
 await i18n.changeLanguage('en');
});


test('recovered current Local artwork reaches PlayerBar, both Now Playing hosts and the exact mixed-queue row without changing queue metadata',()=>{
 const cover='data:image/png;base64,c3ludGhldGlj';
 const current={...snapshot.playback,state:'pause',elapsed:19.5,thumb:cover,current_media:{kind:'local',uri:'Source/Album/one.flac',queue_index:0}};
 const hooks={...React,default:React,useEffect(){},useRef:v=>({current:v}),useState:v=>[v,()=>{}]};
 const {PlayerBar}=compile('components/PlayerBar.tsx',{'react':hooks,'../services/audio':{audioService:{}}});
 assert.equal(nodes(PlayerBar({status:current,health:snapshot.health,route:snapshot.route,onToggleQueue(){},isQueueOpen:false}),n=>n.type==='img')[0].props.src,cover);
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:View},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService:{}},'../services/nowPlayingWindow':{}});
 for(const embedded of [true,false]){
   const host=NowPlayingHost({snapshot:{...snapshot,playback:current},owner,embedded,onReady(){}});
   const html=renderToStaticMarkup(React.createElement(View,host.props));assert.ok(html.includes(cover));assert.equal(host.props.snapshot.playback.elapsed,19.5);
 }
 const items=[{title:'One',artist:'Fixture',album:'Album',uri:'Source/Album/one.flac',thumb:null},{title:'Plex',artist:'Fixture',album:'Other',media_locator:{kind:'plex',server_id:'s',part_key:'p'},plex_image:{server_id:'s',path:'/thumb'}}];
 const before=structuredClone(items);let state=0;
 const queueHooks={...hooks,useState:v=>[[{items,phase:'ready'},{top:0,height:600},false,false][state++]??v,()=>{}]};
 const {NowPlayingQueue}=compile('components/NowPlayingQueue.tsx',{'react':queueHooks,'../services/audio':{audioService:{}}});
 const queue=NowPlayingQueue({open:true,available:true,status:current});const art=nodes(queue,n=>n.type===dependencies['./TrackArtwork'].TrackArtwork);
 assert.equal(art[0].props.track.thumb,cover);assert.equal(art[1].props.track,items[1]);assert.deepEqual(items,before);
 assert.equal(queueItemArtwork(items[0],1,current),items[0],'duplicate URI at a different queue index stays untouched');
 assert.equal(queueItemArtwork(items[0],0,{...current,current_media:{kind:'plex',server_id:'s',part_key:'p',queue_index:0}}),items[0]);
});


test('inactive visualizers show readable guidance, hide idle graphs, and keep keyboard help in every mode',async()=>{
 for(const language of ['en','pt-BR']){
  await i18n.changeLanguage(language);
  for(const mode of ['combo','spectrum-neon','spectrum-segmented','vu-amber','vu-blue']){
   const hooks={...React,default:React,useState:()=>[mode,()=>{}],useEffect(){}};
   const {VisualizerPanel}=compile('components/visualizer/VisualizerPanel.tsx',{'react':hooks});
   const label=i18n.t('nowPlaying.analyzerReason.current_audio_format_is_dsd'),help=i18n.t('nowPlaying.visualizer.helpDsdDirect');
   const tree=VisualizerPanel({levels:props.levels,active:false,inactiveLabel:label,inactiveHelp:help,fullscreen:false});
   const message=nodes(tree,n=>n.props['data-visualizer-inactive']!==undefined)[0];assert.ok(message);assert.equal(message.props.role,'status');
   const messageHtml=renderToStaticMarkup(message);assert.ok(messageHtml.includes(label));assert.ok(messageHtml.includes(help));assert.doesNotMatch(messageHtml,/truncate|text-\[8px\]/);
   assert.doesNotMatch(renderToStaticMarkup(tree),/<svg[^>]*role="img"/);
   const details=nodes(tree,n=>n.type==='details')[0];const summary=nodes(details,n=>n.type==='summary')[0];assert.equal(summary.props['aria-label'],i18n.t('nowPlaying.visualizer.howItWorks'));
   const html=renderToStaticMarkup(details);for(const key of ['helpOverview','worksPcm','worksDsd','directDsd'])assert.ok(html.includes(i18n.t('nowPlaying.visualizer.'+key)));
   let focused=false,prevented=false,stopped=false;const target={open:true,querySelector:()=>({focus(){focused=true;}})};
   details.props.onKeyDown({key:'Escape',currentTarget:target,preventDefault(){prevented=true;},stopPropagation(){stopped=true;}});assert.equal(target.open,false);assert.ok(focused&&prevented&&stopped);
   const active=VisualizerPanel({levels:props.levels,active:true,fullscreen:false});assert.equal(nodes(active,n=>n.props['data-visualizer-inactive']!==undefined).length,0);assert.equal(nodes(active,n=>n.type==='details').length,1);
  }
 }
 await i18n.changeLanguage('en');
});

test('both player hosts explain paused, stopped and unavailable states instead of showing a stale analyzer reason',()=>{
 const panel=dependencies['./visualizer/VisualizerPanel'].VisualizerPanel;
 for(const embedded of [true,false]){
  const cases=[
   [{analyzerAvailable:true,snapshot:{...snapshot,playback:{...snapshot.playback,state:'pause'}}},'nowPlaying.visualizer.paused','nowPlaying.visualizer.helpPaused'],
   [{analyzerAvailable:false,snapshot:{...snapshot,playback:{...snapshot.playback,state:'stop'}}},'nowPlaying.analyzerReason.stopped','nowPlaying.visualizer.helpStopped'],
   [{analyzerAvailable:false,snapshot:{...snapshot,health:{state:'unavailable'}}},'nowPlaying.analyzerReason.mpd_unavailable','nowPlaying.visualizer.helpUnavailable'],
  ];
  for(const [change,label,help]of cases){
   const tree=View({...props,...change,embedded,analyzerInactiveLabel:'old DSD reason',analyzerInactiveHelp:'old DSD help'});
   const visualizer=nodes(tree,n=>n.type===panel)[0];assert.equal(visualizer.props.active,false);assert.equal(visualizer.props.inactiveLabel,i18n.t(label));assert.equal(visualizer.props.inactiveHelp,i18n.t(help));
  }
 }
});

test('host guidance distinguishes blocked DSD in Direct from unavailable DSD in Shared without audio actions',()=>{
 for(const embedded of [true,false])for(const mode of ['direct','shared'])for(const reason of ['current_audio_format_is_dsd','current_file_is_dsd','reader_error',null,'unknown/private?token=fixture']){
  let cursor=0;const states=[false,false,false,0,false,props.levels,false,reason];
  const hooks={...React,default:React,useState:initial=>[states[cursor++]??initial,()=>{}],useEffect(){},useRef:initial=>({current:initial})};
  const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:()=>null},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService:{}},'../services/nowPlayingWindow':{startAudioAnalyzer:()=>assert.fail('unexpected read'),stopAudioAnalyzer:()=>assert.fail('unexpected stop')}});
  const tree=NowPlayingHost({snapshot:{...snapshot,route:{mode,endpoint:mode==='shared'?'default':'hw:Fixture,0'}},owner,embedded,onReady(){}});
  const key=reason?.startsWith('current_')?(mode==='shared'?'helpDsdShared':'helpDsdDirect'):reason?'helpRetry':'helpPreparing';
  assert.equal(tree.props.analyzerInactiveHelp,i18n.t('nowPlaying.visualizer.'+key));assert.ok(!tree.props.analyzerInactiveLabel.includes('token='));
 }
});

test('analyzer start rejection produces an explanatory failure state without retrying or changing playback',async()=>{
 const effects=[],values=[],calls=[];const hooks={...React,default:React,useState:initial=>[initial,value=>values.push(value)],useEffect:fn=>effects.push(fn),useRef:initial=>({current:initial})};
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:()=>null},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{listen:async()=>()=>{}},'@tauri-apps/api/window':{},'../services/audio':{audioService:{}},'../services/nowPlayingWindow':{AUDIO_LEVEL_EVENT:'level',ANALYZER_STATUS_EVENT:'analyzer',startAudioAnalyzer:async()=>{calls.push('start');throw new Error('synthetic');},stopAudioAnalyzer:async()=>calls.push('stop')}});
 NowPlayingHost({snapshot,owner,embedded:true,onReady(){}});const cleanup=effects[2]();await flush();assert.ok(values.includes('initial_reconcile_failed'));assert.deepEqual(calls,['start']);cleanup();await flush();assert.deepEqual(calls,['start','stop']);
});

test('Now Playing hides gradual volume for all Direct transports and retains mute',async()=>{
 for(const lng of ['en','pt-BR']){await i18n.changeLanguage(lng);
  for(const embedded of [false,true])for(const [mode,audio_format,backend,slider] of [
   ['direct','96000:24:2','direct_equipment',false],['direct','dsd64:2','direct_equipment',false],
   ['direct','705600:dsd:2','direct_equipment',false],['direct','dsd128:2','direct_equipment',false],['shared','dsd64:2','pipewire',true]]){
   let muted=0;const tree=View({...props,embedded,onMute:()=>{muted++},snapshot:{...snapshot,route:{mode,endpoint:mode==='direct'?'hw:Fixture,0':'default'},playback:{...snapshot.playback,audio_format,volume:{...snapshot.playback.volume,backend,...(mode==='direct'?{value:100,writable:false,direct_gain:'unity',direct_token:'1:65'}:{})}}}});
   const ranges=nodes(tree,n=>n.type==='input'&&n.props['aria-label']===i18n.t('signalPath.volume'));assert.equal(ranges.length,slider?1:0);
   const mute=nodes(tree,n=>n.type==='button'&&n.props['aria-label']===i18n.t('player.mute'))[0];assert.ok(mute);mute.props.onClick();assert.equal(muted,1);
   if(!slider)assert.ok(renderToStaticMarkup(tree).includes(i18n.t('player.volumeOnDevice')));
  }
 }await i18n.changeLanguage('en');
});

test('embedded and detached hosts use explicit mute without an independent preparation flow',async()=>{
 const hooks={...React,default:React,useState:initial=>[initial,()=>{}],useEffect(){},useRef:initial=>({current:initial})};
 for(const embedded of [true,false])for(const [gain,value,muted]of [['pending',0,false],['unity',100,false],['muted',0,true],['changed',43,false]]){
  const calls=[];window.confirm=()=>assert.fail('no prepare ritual');
  const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:()=>null},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService:{setDirectMute:async r=>calls.push(r),setVolume:()=>assert.fail()}},'../services/nowPlayingWindow':{}});
  const tree=NowPlayingHost({snapshot:{...snapshot,route:{mode:'direct',endpoint:'hw:Fixture,0'},playback:{...snapshot.playback,volume:{backend:'direct_equipment',available:true,writable:false,value,muted,direct_gain:gain,direct_token:'old:1'}}},owner,embedded,onReady(){}});
  tree.props.onMute();await flush();assert.deepEqual(calls,[{muted:!muted,token:'old:1'}]);
 }delete window.confirm;
});

 test('Now Playing Shared unmute uses backend context instead of an independent default volume',async()=>{
  const calls=[];
  const hooks={...React,default:React,useState:initial=>[initial,()=>{}],useEffect(){},useRef:initial=>({current:initial})};
  const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{'react':hooks,'./NowPlayingPresentation':{NowPlayingPresentation:()=>null},'./NowPlayingQueue':{NowPlayingQueue:()=>null},'@tauri-apps/api/event':{},'@tauri-apps/api/window':{},'../services/audio':{audioService:{setSharedMute:async request=>calls.push(request),setVolume:()=>assert.fail('must not choose a level')}},'../services/nowPlayingWindow':{}});
  const owner={host:'main',session:'fixture',generation:1};
  const tree=NowPlayingHost({snapshot:{...snapshot,playback:{...snapshot.playback,volume:{value:0,muted:true,writable:true,available:true,backend:'mpd_software',shared_token:'shared-session'}}},owner,embedded:true,onReady(){}});
  tree.props.onMute();await flush();assert.deepEqual(calls,[{muted:false,token:'shared-session'}]);
 });


test('actual App and PlayerBar callbacks retain a decoded cover across reopening and same-image track changes',()=>{
 const source=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');
 const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
 let player,configure;
 const walk=node=>{
  if(ts.isJsxSelfClosingElement(node)&&node.tagName.getText(ast)==='PlayerBar')player=node;
  if(ts.isVariableDeclaration(node)&&node.name.getText(ast)==='setConfig')configure=node.initializer.arguments[0].getText(ast);
  ts.forEachChild(node,walk);
 };walk(ast);assert.ok(player&&configure);
 const jpeg='data:image/jpeg;base64,'+Buffer.from([255,216,255,192,0,11,8,0,80,0,80,1,1,17,0,255,217]).toString('base64');
 let value=null,reductions=0;
 const storage={getItem:()=>value,setItem:(_key,next)=>{value=next},removeItem:()=>{value=null}};
 const cache=new LocalPlaybackThumbnail(()=>storage,()=>{reductions++;return jpeg});
 const configFn=ts.transpileModule('exports.configure = '+configure,{compilerOptions:{module:ts.ModuleKind.CommonJS}}).outputText;
 const configExports={};new Function('exports','localPlaybackThumbnail','localAlbumThumbnails','localAlbumCatalog','setConfigState',configFn)(configExports,cache,{setContext(){}},{setPresentationContext(){}},()=>{});
 configExports.configure({first_run:false,local_folders:['/music']});
 const callback=name=>new Function('localPlaybackThumbnail','return ('+player.attributes.properties.find(p=>p.name?.getText(ast)===name).initializer.expression.getText(ast)+')')(cache);
 const loaded=callback('onArtworkLoaded'),failed=callback('onArtworkFailed');
 const hooks={...React,default:React,useEffect(){},useRef:v=>({current:v}),useState:v=>[v,()=>{}]};
 const {PlayerBar}=compile('components/PlayerBar.tsx',{'react':hooks,'../services/audio':{audioService:{}}});
 const image={src:'data:image/png;base64,AAAA',complete:true,naturalWidth:80,naturalHeight:80};
 let current={...snapshot.playback,current_media:{kind:'local',uri:'Source/Album/one.flac',queue_index:0},thumb:image.src};
 const render=()=>nodes(PlayerBar({status:current,health:snapshot.health,route:snapshot.route,isQueueOpen:false,onToggleQueue(){},onArtworkLoaded:loaded,onArtworkFailed:failed}),n=>n.type==='img')[0];
 render().props.onLoad({currentTarget:image});assert.equal(cache.get(current.current_media),jpeg);
 const restored=new LocalPlaybackThumbnail(()=>storage);restored.setContext(['/music']);
 assert.equal(restored.get(current.current_media),jpeg,'the next session needs no image or metadata lookup');
 current={...current,current_media:{...current.current_media,uri:'Source/Album/two.flac',queue_index:1}};
 render().props.ref(image);assert.equal(cache.get(current.current_media),jpeg,'already decoded unchanged src is retained for the new track too');
 for(let n=0;n<10;n++)render().props.ref(image);
 assert.equal(reductions,2,'status renders do not redraw canvas or write storage repeatedly');
 current={...current,thumb:jpeg};render().props.onError();assert.equal(cache.get(current.current_media),null);
 configExports.configure({first_run:true,local_folders:['/music']});assert.equal(cache.get(current.current_media),null);
});
