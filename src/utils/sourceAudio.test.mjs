import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import {sourceAudioBadge,albumAudioBadge,PlayerSourceReader} from './sourceAudio.ts';
import {currentMediaKey} from './playerNavigation.ts';
const flush=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{resolve,reject,promise};};
const pcm=(extra={})=>({provenance:'local',codec:'FLAC',compression:'lossless',sample_type:'integer',sample_rate:96000,bit_depth:24,dsd_level:null,revision:'fixture-r1',...extra});
const dsd=(level=64)=>pcm({codec:'DSD',compression:'lossless',sample_type:'dsd',sample_rate:44100*level,bit_depth:1,dsd_level:level});
const media=(uri='Music/a.flac')=>({kind:'local',uri,queue_index:0});
const result=(m=media(),info=pcm())=>({media:m,info});

test('source classification requires original integer lossless depth AND rate; source DSD is Hi-Res',()=>{
 for(const provenance of ['local','plex']){
  for(const [bit_depth,sample_rate,expected] of [[16,96000,false],[24,48000,false],[24,88200,false],[24,96000,true],[32,192000,true]])assert.equal(sourceAudioBadge(pcm({provenance,bit_depth,sample_rate})).hiRes,expected);
  for(const extra of [{codec:'MP3',compression:'lossy'},{codec:'AAC',compression:'lossy'},{codec:'WavPack',compression:'hybrid'},{compression:'unknown'},{sample_type:'float'},{sample_type:'not_applicable'},{bit_depth:null},{sample_rate:null},{sample_rate:'96000'},{bit_depth:Infinity},{bit_depth:24.5}])assert.equal(sourceAudioBadge(pcm({provenance,...extra})).hiRes,false,JSON.stringify(extra));
  for(const level of [64,128,256]){const badge=sourceAudioBadge({...dsd(level),provenance});assert.equal(badge.label,`DSD${level}`);assert.equal(badge.hiRes,true);}
  for(const extra of [{dsd_level:null},{sample_rate:176400},{sample_rate:352800},{sample_rate:705600},{dsd_level:512}])assert.equal(sourceAudioBadge({...dsd(),...extra,provenance}).hiRes,false);
 }
 assert.equal(sourceAudioBadge(pcm({codec:'M4A'})),null);assert.equal(sourceAudioBadge(pcm({codec:'unknown'})),null);assert.equal(sourceAudioBadge(null),null);
});

test('album summary requires every complete fact; mixed codecs/DSD levels stay mixed, all DSD earns Hi-Res',()=>{
 assert.equal(albumAudioBadge([pcm(),pcm()]).label,'FLAC');assert.equal(albumAudioBadge([pcm(),pcm()]).hiRes,true);
 assert.equal(albumAudioBadge([dsd(64),dsd(64)]).label,'DSD64');assert.equal(albumAudioBadge([dsd(64),dsd(128)]).label,'mixed');assert.equal(albumAudioBadge([dsd(64),dsd(128)]).hiRes,true);
 for(const fact of [pcm({codec:'MP3',compression:'lossy'}),pcm({bit_depth:16}),pcm({compression:'unknown'}),pcm({sample_rate:null})])assert.equal(albumAudioBadge([pcm(),fact]).hiRes,false);
 assert.equal(albumAudioBadge([pcm(),pcm({codec:'AAC',compression:'lossy'})]).label,'mixed');
 for(const facts of [[],null,[pcm(),null],[pcm(),pcm({codec:'unknown'})],[pcm(),pcm({provenance:'plex'})],Array(33).fill(pcm())])assert.equal(albumAudioBadge(facts),null);
});

test('current source acquisition deduplicates repeated telemetry and hosts, drops late replies, and bounds pending work',async()=>{
 const jobs=[];const reader=new PlayerSourceReader(m=>{const job={m,...deferred()};jobs.push(job);return job.promise;});const context={};
 reader.setContext(media(),context);for(let i=0;i<200;i++)reader.setContext(media(),context);await flush();assert.equal(jobs.length,1);
 reader.setContext(media('b'),context);reader.setContext(media('c'),context);reader.setContext(media('d'),context);await flush();assert.equal(jobs.length,1);assert.equal(reader.getSnapshot(),null);
 jobs[0].resolve(result());await flush();assert.equal(jobs.length,2);assert.equal(jobs[1].m.uri,'d');assert.equal(reader.getSnapshot(),null);
 jobs[1].resolve(result(media('d')));await flush();const shared=reader.getSnapshot();assert.ok(shared);reader.setContext(media('d'),context);await flush();assert.equal(reader.getSnapshot(),shared);assert.equal(jobs.length,2);
 const failedContext={};reader.setContext(media('d'),failedContext);assert.equal(reader.getSnapshot(),null);await flush();jobs[2].reject(new Error('private raw detail'));await flush();reader.setContext(media('d'),failedContext);assert.equal(jobs.length,3);
 reader.setContext(null,context);await flush();assert.equal(reader.getSnapshot(),null);
});

test('source unavailable/mismatched results never publish or retry until identity/context changes',async()=>{
 let calls=0;const reader=new PlayerSourceReader(async()=>{calls++;return calls===1?null:result(media('wrong'));});const ctx={};reader.setContext(media(),ctx);await flush();for(let i=0;i<10;i++)reader.setContext(media(),ctx);await flush();assert.equal(calls,1);assert.equal(reader.getSnapshot(),null);
 reader.setContext(media('new'),ctx);await flush();assert.equal(calls,2);assert.equal(reader.getSnapshot(),null);
});

const i18n=i18next.createInstance();await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(lng=>[lng,{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}]))});
const deps={react:{...React,default:React},'react/jsx-runtime':await import('react/jsx-runtime'),'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'lucide-react':icons,'../utils/sourceAudio':{sourceAudioBadge,albumAudioBadge},'../utils/playerNavigation':{currentMediaKey}};
const compile=(path,extra={})=>{const out={};new Function('require','exports',ts.transpileModule(readFileSync(new URL(path,import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText)(name=>{assert.ok(name in extra||name in deps,name);return extra[name]??deps[name];},out);return out;};
const {SourceAudioBadges}=compile('../components/SourceAudioBadges.tsx');deps['./SourceAudioBadges']={SourceAudioBadges};
const hooks={...React,default:React,useEffect(){},useState:v=>[v,()=>{}],useRef:v=>({current:v})};
const nullView=()=>null;
const {PlayerBar}=compile('../components/PlayerBar.tsx',{'react':hooks,'./PlaybackOptionsControls':{PlaybackOptionsControls:nullView},'./SignalPath':{SignalPath:nullView},'./PlexImage':{PlexImage:nullView},'../utils/frontendDiagnostics':{logError(){}},'../services/audio':{audioService:new Proxy({},{get(){return()=>assert.fail('unexpected playback command');}})}});
const {NowPlayingPresentation}=compile('../components/NowPlayingPresentation.tsx',{'./TrackArtwork':{TrackArtwork:nullView},'./SignalPath':{SignalPath:nullView},'./visualizer/VisualizerPanel':{VisualizerPanel:nullView},'./nowPlaying.css':{}});
const playback={title:'Fixture',artist:'Artist',album:'Album',state:'play',elapsed:1,duration:100,audio_format:'192000:32:2',current_media:media(),volume:{backend:'pipewire',available:true,writable:true,value:50},is_updating:false};
const health={state:'available'};
test('real badges have stable text/color, translated source/limits and focus tooltip; cards do not capture actions',async()=>{
 for(const lng of ['en','pt-BR']){await i18n.changeLanguage(lng);
  const html=renderToStaticMarkup(React.createElement(SourceAudioBadges,{info:dsd()}));assert.match(html,/DSD64/);assert.match(html,/Hi-Res/);assert.match(html,/text-rose-200/);assert.match(html,/text-amber-200/);assert.match(html,/tabindex="0"/);assert.match(html,/role="tooltip"/);assert.ok(html.includes(i18n.t('sourceAudio.local')));assert.ok(html.includes(i18n.t('sourceAudio.limit')));
  const compact=renderToStaticMarkup(React.createElement(SourceAudioBadges,{album:[pcm(),pcm({codec:'MP3',compression:'lossy'})],compact:true}));assert.ok(compact.includes(i18n.t('sourceAudio.mixed')));assert.doesNotMatch(compact,/tabindex|<button|role="tooltip"|>Hi-Res</);
  assert.equal(renderToStaticMarkup(React.createElement(SourceAudioBadges,{info:null})), '');
 }
});
test('three real player presentations show identical source facts and immediately hide wrong/absent/unavailable ones',()=>{
 for(const info of [pcm(),dsd()])for(const sourceInfo of [result(media(),info),result(media('old'),info),null])for(const available of [true,false]){
  const h=available?health:{state:'unavailable',reason:'socket_unavailable'};
  const bar=renderToStaticMarkup(PlayerBar({status:playback,sourceInfo,health:h,route:null,isQueueOpen:false,onToggleQueue(){}}));
  const expected=available&&sourceInfo?.media.uri===media().uri;
  assert.equal(bar.includes('data-source-audio'),!!expected);
  for(const embedded of [true,false]){
   const expanded=renderToStaticMarkup(React.createElement(NowPlayingPresentation,{snapshot:{playback,health:h,route:null,sourceInfo},embedded,levels:{spectrum:[]},queue:null,queueOpen:false,analyzerAvailable:false,analyzerInactiveLabel:'fixture',seekValue:1}));
   assert.equal(expanded.includes('data-source-audio'),!!expected);if(expected){assert.ok(expanded.includes(info.codec==='DSD'?'DSD64':'FLAC'));assert.match(expanded,/>Hi-Res</);}
  }
 }
});

test('shared physical source gate prioritizes the player, deduplicates queued kind and skips obsolete offscreen work',async()=>{
 const {SourceReadGate}=await import('./sourceReadGate.ts');const gate=new SourceReadGate(),calls=[],job=deferred();let visible=true;
 const album=gate.run('album',async()=>{calls.push('album');return 'album';},()=>visible);
 const player=gate.run('player',async()=>{calls.push('player');return job.promise;},()=>true);
 await flush();assert.deepEqual(calls,['player']);visible=false;job.resolve('player');assert.equal(await player,'player');assert.equal(await album,null);assert.deepEqual(calls,['player']);
 const first=gate.run('album',async()=>assert.fail('replaced queued work'),()=>true);const last=gate.run('album',async()=>{calls.push('last');return 'last';},()=>true);
 assert.equal(await first,null);assert.equal(await last,'last');assert.deepEqual(calls,['player','last']);
});


test('player source reader rejects wrong provenance',async()=>{
 const player=new PlayerSourceReader(async()=>result(media(),pcm({provenance:'plex'})));player.setContext(media(),{});await flush();assert.equal(player.getSnapshot(),null);

});

test('App source context includes confirmed Plex catalog revisions as well as Local revisions and connection identity',()=>{
 const app=readFileSync(new URL('../App.tsx',import.meta.url),'utf8');const start=app.indexOf('const sourceInfoContext = useMemo');const declaration=app.slice(start,app.indexOf(';',start));
 for(const dependency of ['plexBrowserCatalog','localCatalogRevision','localCatalog.albums','plexConnectionIdentity','config?.local_folders','config?.plex_token'])assert.ok(declaration.includes(dependency),dependency);
});
