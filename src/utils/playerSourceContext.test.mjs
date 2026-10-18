import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import i18next from 'i18next';
import {PlayerSourceReader,sourceAudioBadge,albumAudioBadge} from './sourceAudio.ts';
import {SourceReadGate} from './sourceReadGate.ts';
import {currentMediaKey} from './playerNavigation.ts';
import {requiresDeviceVolumeControl} from './signalPath.ts';
import {LocalAlbumCatalog} from './localAlbumCatalog.ts';
const read=f=>readFileSync(new URL(f,import.meta.url),'utf8');
const flush=async()=>{for(let i=0;i<60;i++)await Promise.resolve();};
const deferred=()=>{let resolve;return {promise:new Promise(r=>resolve=r),resolve:v=>resolve(v)};};
function compile(file,deps){const out={};new Function('require','exports',ts.transpileModule(read(file),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},out);return out;}
const app=read('../App.tsx'),ast=ts.createSourceFile('App.tsx',app,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX),declarations=new Map();let snapshotEffect;
function visit(n){if(ts.isVariableDeclaration(n)&&['playerSourceKind','sourceInfoContext','sourceInfo'].includes(n.name.getText(ast)))declarations.set(n.name.getText(ast),n.initializer.getText(ast));if(ts.isCallExpression(n)&&n.expression.getText(ast)==='useEffect'&&n.arguments[0]?.getText(ast).includes('nowPlayingSnapshotRef.current ='))snapshotEffect=n.arguments[0].getText(ast);ts.forEachChild(n,visit);}visit(ast);assert.ok(snapshotEffect&&declarations.has('sourceInfo'));
const vars=['config','configSavePhase','isPlaybackAvailable','playbackStatus','localCatalog','plexConnectionIdentity','plexBrowserCatalog','localCatalogRevision'];
const project=new Function('scope','useMemo','usePlayerSourceInfo',`const {${vars}}=scope;${[...declarations].map(([n,v])=>`const ${n}=${v};`).join('')}return sourceInfo;`);
const plex=(n=1)=>({kind:'plex',server_id:'fixture-server',part_key:`/library/parts/${n}/file`,queue_index:n-1});
const local=()=>({kind:'local',uri:'Music/1.flac',queue_index:0});
const pcm=(extra={})=>({provenance:'plex',codec:'FLAC',compression:'lossless',sample_type:'integer',sample_rate:96000,bit_depth:24,dsd_level:null,revision:null,...extra});
const response=(media,info=pcm())=>info?{media,info}:null;
function mount({media=plex(),readInfo=async m=>response(m),updating=false}={}){
 const slots=[],effects=[],calls=[];let cursor=0;const changed=(a,b)=>!a||a.length!==b.length||a.some((v,i)=>!Object.is(v,b[i]));
 const hooks={useMemo(fn,deps){const i=cursor++;if(!slots[i]||changed(slots[i].deps,deps))slots[i]={deps,value:fn()};return slots[i].value;},useSyncExternalStore(subscribe,get){return get();},useLayoutEffect(fn,deps){const i=cursor++,old=slots[i];if(changed(old?.deps,deps))effects.push(()=>{old?.cleanup?.();slots[i]={deps,fn,cleanup:fn()};});}};
 const service=compile('../services/sourceAudio.ts',{'@tauri-apps/api/core':{invoke:async(command,args)=>{assert.equal(command,'get_player_source_info');calls.push(args.media);return readInfo(args.media);}},'../utils/sourceReadGate':{SourceReadGate}});
 const hook=compile('./usePlayerSourceInfo.ts',{react:hooks,'../services/sourceAudio':service,'./playerNavigation':{currentMediaKey},'./sourceAudio':{PlayerSourceReader}}).usePlayerSourceInfo;
 const catalog=new LocalAlbumCatalog(async()=>[]);
 const scope={config:{first_run:false,local_folders:['Fixture'],plex_server_id:'fixture-server',plex_token:'synthetic-secret'},configSavePhase:null,isPlaybackAvailable:true,playbackStatus:{current_media:media,state:'play',title:'Fixture',artist:'Artist',album:'Album',duration:10,elapsed:1,audio_format:'192000:32:2',volume:{backend:'software',available:true,writable:true,value:100},is_updating:updating},localCatalog:catalog.getSnapshot(),localCatalogRevision:0,plexConnectionIdentity:{},plexBrowserCatalog:{}};
 const f={scope,calls,catalog,render(){cursor=0;scope.localCatalog=catalog.getSnapshot();f.info=project(scope,hooks.useMemo,hook);effects.splice(0).forEach(fn=>fn());return f.info;},replay(){for(const s of slots)if(s?.fn){s.cleanup?.();s.cleanup=s.fn();}},close(){slots.forEach(s=>s?.cleanup?.());},snapshot(){let value;const v={config:scope.config,mpdHealth:{state:scope.isPlaybackAvailable?'available':'unavailable'},presentationPlayback:scope.playbackStatus,sourceInfo:f.info,queueArtwork:{blocked:true,onlineEnabled:false,generation:0},nowPlayingSnapshotRef:{current:null},configuredSignalRoute:()=>({mode:'shared',endpoint:'default'}),publishCurrentNowPlayingSnapshot:()=>value=v.nowPlayingSnapshotRef.current};new Function(...Object.keys(v),`return (${snapshotEffect})();`)(...Object.values(v));return value;}};return f;
}
async function settle(f){f.render();await flush();f.render();await flush();return f.render();}
const i18n=i18next.createInstance();await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(lng=>[lng,{translation:JSON.parse(read(`../locales/${lng}.json`))}]))});
const noop=()=>null,renderHooks={...React,default:React,useState:v=>[v,()=>{}],useEffect(){},useRef:v=>({current:v})};
const deps={react:{...React,default:React},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'lucide-react':icons,'../utils/sourceAudio':{sourceAudioBadge,albumAudioBadge},'../utils/playerNavigation':{currentMediaKey},'../utils/signalPath':{requiresDeviceVolumeControl},'./SignalPath':{SignalPath:noop},'./PlexImage':{PlexImage:noop},'./PlaybackOptionsControls':{PlaybackOptionsControls:noop},'../utils/frontendDiagnostics':{logError(){}},'../services/audio':{audioService:new Proxy({},{get(){return()=>assert.fail('labels cannot command audio');}})}};
const {SourceAudioBadges}=compile('../components/SourceAudioBadges.tsx',deps);deps['./SourceAudioBadges']={SourceAudioBadges};
const {PlayerBar}=compile('../components/PlayerBar.tsx',{...deps,react:renderHooks});
const {NowPlayingPresentation}=compile('../components/NowPlayingPresentation.tsx',{...deps,'./TrackArtwork':{TrackArtwork:noop},'./visualizer/VisualizerPanel':{VisualizerPanel:noop},'./nowPlaying.css':{}});
function html(f){const snapshot=f.snapshot();return [renderToStaticMarkup(PlayerBar({status:snapshot.playback,health:snapshot.health,route:snapshot.route,sourceInfo:snapshot.sourceInfo,isQueueOpen:false,onToggleQueue(){}})),...[true,false].map(embedded=>renderToStaticMarkup(React.createElement(NowPlayingPresentation,{snapshot,embedded,queue:null,queueOpen:false,levels:{spectrum:[]},seekValue:1,analyzerAvailable:false,analyzerInactiveLabel:'Fixture'})))];}
function badges(f,expected,label='FLAC',hiRes=true){for(const h of html(f)){assert.equal(h.includes('data-source-audio='),expected);if(expected){assert.ok(h.includes(`>${label}</`));assert.equal(h.includes('>Hi-Res</'),hiRes);}}}

test('actual App/hook/IPC acquires Plex facts during Local indexing and shares labels in bar, embedded and detached players',async()=>{
 for(const lng of ['en','pt-BR']){await i18n.changeLanguage(lng);const f=mount({updating:true});f.catalog.waitForMpd();f.catalog.setSaveBlocked(true);await settle(f);assert.equal(f.calls.length,1,'Local indexing must not suppress Plex source IPC');badges(f,true);const value=f.info;f.scope.playbackStatus={...f.scope.playbackStatus,state:'pause'};await settle(f);assert.equal(f.info,value);badges(f,true);for(let i=0;i<10;i++)html(f);assert.equal(f.calls.length,1,'hosts and pause must not duplicate requests');f.close();}
});
test('Local activity cannot erase known Plex facts or discard an in-flight matching response',async()=>{
 for(const delayed of [false,true]){const d=deferred(),f=mount({readInfo:m=>delayed?d.promise:response(m)});f.render();await flush();if(!delayed){f.render();badges(f,true);}f.scope.playbackStatus={...f.scope.playbackStatus,is_updating:true};f.catalog.waitForMpd();f.scope.localCatalogRevision++;f.scope.config={...f.scope.config,local_folders:['Other fixture']};f.render();if(delayed){d.resolve(response(plex()));await flush();}await settle(f);badges(f,true);assert.equal(f.calls.length,1);f.scope.playbackStatus={...f.scope.playbackStatus,is_updating:false};f.catalog.setSaveBlocked(true);await settle(f);badges(f,true);f.catalog.setSaveBlocked(false);await settle(f);badges(f,true);assert.equal(f.calls.length,1);f.close();}
});
test('Local facts retain their indexing gate and relevant revisions, without interference from Plex context',async()=>{
 const f=mount({media:local(),updating:true,readInfo:async m=>response(m,pcm({provenance:'local'}))});f.catalog.waitForMpd();await settle(f);assert.equal(f.calls.length,0);badges(f,false);f.scope.playbackStatus={...f.scope.playbackStatus,is_updating:false};f.catalog.setUpdating(false,true);await settle(f);assert.equal(f.calls.length,1);badges(f,true);const value=f.info;f.scope.plexConnectionIdentity={};f.scope.plexBrowserCatalog={};f.scope.config={...f.scope.config,plex_token:'other-synthetic'};await settle(f);assert.equal(f.info,value);assert.equal(f.calls.length,1);f.scope.localCatalogRevision++;assert.equal(f.render(),null);await settle(f);badges(f,true);assert.equal(f.calls.length,2);f.close();
});
test('late A never appears for B; only meaningful Plex contexts reacquire, not pause or telemetry',async()=>{
 const a=deferred(),b=deferred(),f=mount({updating:true,readInfo:m=>m.queue_index===0?a.promise:b.promise});f.render();await flush();assert.equal(f.calls.length,1);f.scope.playbackStatus={...f.scope.playbackStatus,current_media:plex(2),state:'pause'};f.render();badges(f,false);a.resolve(response(plex()));await flush();f.render();assert.equal(f.calls.length,2);badges(f,false);b.resolve(response(plex(2)));await settle(f);badges(f,true);for(let i=0;i<20;i++){f.scope.playbackStatus={...f.scope.playbackStatus,elapsed:i};await settle(f);}assert.equal(f.calls.length,2);for(const key of ['plexConnectionIdentity','plexBrowserCatalog']){f.scope[key]={};assert.equal(f.render(),null);await settle(f);badges(f,true);}assert.equal(f.calls.length,4);f.close();
});
test('known format does not imply Hi-Res; unknown/wrong identity/busy never guess or retry indefinitely',async()=>{
 const facts=[[pcm(),'FLAC',true],[pcm({bit_depth:16,sample_rate:44100}),'FLAC',false],[pcm({sample_rate:48000}),'FLAC',false],[pcm({codec:'MP3',compression:'lossy',sample_type:'not_applicable'}),'MP3',false],[pcm({codec:'PCM float',compression:'uncompressed',sample_type:'float'}),'PCM float',false],[pcm({codec:'WavPack',compression:'hybrid'}),'WavPack',false],...[64,128,256].map(n=>[pcm({codec:'DSD',sample_type:'dsd',sample_rate:44100*n,dsd_level:n,bit_depth:1}),`DSD${n}`,true]),[pcm({codec:'DSD',sample_type:'dsd',sample_rate:352800,dsd_level:null}),'DSD',false]];
 for(const [info,label,hiRes] of facts){const f=mount({updating:true,readInfo:async m=>response(m,info)});await settle(f);badges(f,true,label,hiRes);f.close();}
 for(const value of [null,response(plex(9)),response(plex(),pcm({provenance:'local'})),response(plex(),pcm({codec:'unknown'}))]){const f=mount({updating:true,readInfo:async()=>value});await settle(f);badges(f,false);for(let i=0;i<10;i++)await settle(f);assert.equal(f.calls.length,1);f.close();}
 // The existing optional IPC uses null for Busy/unknown. Keep bounded negative
 // caching; unrelated Local revisions must not turn it into a retry loop.
 let reads=0;const f=mount({updating:true,readInfo:async m=>++reads===1?null:response(m)});await settle(f);badges(f,false);f.scope.localCatalogRevision++;await settle(f);assert.equal(reads,1);f.scope.plexConnectionIdentity={};await settle(f);assert.equal(reads,2);badges(f,true);f.close();
});
test('global configuration/health/lifecycle guards still reject obsolete work; StrictMode does not duplicate IPC',async()=>{
 for(const gate of ['config','save','health']){const d=deferred(),f=mount({readInfo:()=>d.promise,updating:true});f.render();await flush();assert.equal(f.calls.length,1);if(gate==='config')f.scope.config=null;else if(gate==='save')f.scope.configSavePhase='persisting';else f.scope.isPlaybackAvailable=false;assert.equal(f.render(),null);d.resolve(response(plex()));await flush();f.render();badges(f,false);f.close();}
 const f=mount({updating:true});f.render();f.replay();await settle(f);assert.equal(f.calls.length,1);badges(f,true);f.close();await flush();
});
