import { createLocalCoverBatch } from "./localCoverBatch.ts";
import { createLocalSourceReadQueue } from "./localSourceReadQueue.ts";
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import i18next from 'i18next';
import * as intents from './playbackIntent.ts';
import {playPlexAlbum} from './plexAlbumPlayback.ts';
import {createLocalAlbumPlayback} from './localAlbumPlayback.ts';
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});void promise.catch(()=>{});return{promise,resolve,reject}};
const flush=async()=>{for(let n=0;n<40;n++)await Promise.resolve()};
const track={rating_key:'track',title:'Synthetic track',duration_ms:1000,media_locator:{kind:'plex',server_id:'fixture',part_key:'/fixture-part'}};
const album={rating_key:'album',server_id:'fixture',library_id:'2',title:'Synthetic album',artist:'Fixture'};
let backend=0;
function reset(){intents.configurePlaybackIntents(async()=>String(++backend),async token=>{if(String(backend)===token)backend++},async token=>String(backend)===token)}

test('real helpers admit before the first await; newer Plex, Local or playlist intent wins even if it fails',async()=>{
 for(const kind of ['plex','local','playlist-failure']){
  reset();const slow=deferred(),played=[];
  const a=playPlexAlbum(album,()=>slow.promise,async m=>played.push(m[0].title),()=>true);
  if(kind==='plex')await playPlexAlbum(album,async()=>[{...track,title:'New'}],async m=>played.push(m[0].title),()=>true);
  if(kind==='local'){const local=createLocalAlbumPlayback(async()=>[{disc:{number:1},tracks:[{path:'new.flac',name:'New'}]}],async()=>null,async m=>played.push(m[0].title));await local.play({id:'new',title:'New',artist:'Fixture'},()=>true)}
  if(kind==='playlist-failure'){const newest=intents.beginPlaybackIntent();await intents.playbackIntentToken(newest);/* failed preparation must not revive A */}
  slow.resolve([track]);await a;assert.deepEqual(played,kind==='playlist-failure'?[]:['New']);
 }
});

test('backend rejection and navigation cancellation never resume a stale preparation or leak an unhandled rejection',async()=>{
 reset();const old=intents.beginPlaybackIntent(),newest=intents.beginPlaybackIntent();await newest.token;
 assert.equal(old.current(),false);assert.equal(await old.confirmCurrent(),false);
 old.cancel();assert.equal(await newest.confirmCurrent(),true);
 newest.cancel();await flush();assert.equal(await newest.confirmCurrent(),false);
 intents.configurePlaybackIntents(async()=>{throw 'CONFIG_SAVE_BUSY'},async()=>{});
 const failed=intents.beginPlaybackIntent();await flush();await assert.rejects(intents.playbackIntentToken(failed),e=>e==='CONFIG_SAVE_BUSY');assert.equal(await failed.confirmCurrent(),true);
 reset();
});

const compile=(file,deps)=>{const result={};new Function('require','exports',ts.transpileModule(readFileSync(file,'utf8'),{fileName:file.pathname,compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(id=>{if(id==="./config")return {ensureDirectOutputChoice:async()=>{}}; if(id==="../utils/localCoverBatch")return {createLocalCoverBatch}; if(id==="../utils/localSourceReadQueue")return {createLocalSourceReadQueue,localSourceReadQueue:createLocalSourceReadQueue()}; assert.ok(id in deps,id);return deps[id]},result);return result};
const root=new URL('../',import.meta.url);
const source=file=>new URL(file,root);
const nodes=tree=>React.isValidElement(tree)?[tree,...nodes(tree.props.children)]:Array.isArray(tree)?tree.flatMap(nodes):[];
const same=(a,b)=>a?.length===b?.length&&a.every((v,i)=>Object.is(v,b[i]));
for(const lang of ['pt-BR','en'])for(const name of ['AlbumView','AlbumDetailModal'])test(`${name} real component/hook shows safe errors, resets on success/context and Info never starts playback (${lang})`,async()=>{
 reset();let host;const requests=[];
 const hooks={
  useState(initial){const h=host,i=h.cursor++;h.slots[i]??={value:typeof initial==='function'?initial():initial};const slot=h.slots[i];return[slot.value,value=>{slot.value=typeof value==='function'?value(slot.value):value;h.writes++}]},
  useRef(value){return host.slots[host.cursor++]??={current:value}},
  useMemo(fn,deps){const h=host,i=h.cursor++,old=h.slots[i];if(!old||!same(old.deps,deps))h.slots[i]={deps,value:fn()};return h.slots[i].value},
  useEffect(fn,deps){const h=host,i=h.cursor++,old=h.slots[i];if(!old||!same(old.deps,deps))h.effects.push(()=>{old?.cleanup?.();h.slots[i]={deps,cleanup:fn()}})},
  useLayoutEffect(fn,deps){this.useEffect(fn,deps)},
 };
 const i18n=i18next.createInstance();await i18n.init({lng:lang,resources:{[lang]:{translation:JSON.parse(readFileSync(source(`locales/${lang}.json`),'utf8'))}}});
 const deps={react:{...React,...hooks,default:React},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n),i18n})},'lucide-react':icons,'./playbackIntent':intents,'./frontendDiagnostics':{logError(){}},'../utils/frontendDiagnostics':{logError(){}}};
 // React calls a detached hook function, so avoid relying on method receiver.
 hooks.useLayoutEffect=hooks.useEffect;deps.react.useLayoutEffect=hooks.useEffect;
 const hook=compile(source('utils/usePlaybackAttempt.ts'),deps);
 const notice=compile(source('components/PlaybackRequestNotice.tsx'),{...deps});
 Object.assign(deps,{'../utils/usePlaybackAttempt':hook,'./PlaybackRequestNotice':notice,'../services/audio':{audioService:{playTracks(){const d=deferred();requests.push(d);return d.promise}}},'../services/plex':{plexService:{getAlbumTracks:async()=>[track]}},'../services/favorites':{favoritesService:{getFavorites:async()=>[]}},'./PlexImage':{PlexImage:()=>null},'./PlaylistPickerModal':{PlaylistPickerModal:()=>null},'./PlexFileInfoModal':{PlexFileInfoModal:()=>null}});
 const View=compile(process.env.SONANTE_BASE_UI?new URL(`file://${process.env.SONANTE_BASE_UI}/src/components/${name}.tsx`):source(`components/${name}.tsx`),deps)[name];
 const h={slots:[],cursor:0,effects:[],writes:0,props:{album,active:true,isPlaybackAvailable:true,onBack(){},onClose(){}}};
 const render=()=>{host=h;h.cursor=0;h.effects=[];const tree=View(h.props);h.effects.splice(0).forEach(fn=>fn());return tree};
 const ready=async()=>{render();await flush();return render()};
 const alert=()=>{const tree=render();const item=nodes(tree).find(n=>n.type===notice.PlaybackRequestNotice);return item?.props.failure?renderToStaticMarkup(item):''};
 const click=()=>{const tree=render();const button=nodes(tree).find(n=>n.type==='button'&&nodes(n).some(child=>child.type===icons.Play));assert.ok(button);button.props.onClick()};
 await ready();let tree=render();const beforeInfo=backend;nodes(tree).find(n=>n.props['data-plex-file-info']).props.onClick({stopPropagation(){}});assert.equal(requests.length,0);assert.equal(backend,beforeInfo);
 click();requests.at(-1).reject('FIRST_RUN_CATALOG_INDEXING');await flush();assert.ok(alert().includes(i18n.t('player.requestError.initialIndexing')));assert.doesNotMatch(alert(),/FIRST_RUN_CATALOG_INDEXING/);
 click();requests.at(-1).reject('PLAYBACK_QUEUE_UNCERTAIN');await flush();assert.ok(alert().includes(i18n.t('player.requestError.uncertain')));assert.doesNotMatch(alert(),/fixture-part|X-Plex-Token|ACK/);
 h.props={...h.props,album:{...album,rating_key:'visit-B'}};await ready();assert.equal(alert(),'');h.props={...h.props,album};await ready();assert.equal(alert(),'','an old visible error must not return on A-B-A navigation');
 click();assert.equal(alert(),'');requests.at(-1).resolve();await flush();assert.equal(alert(),'');
 click();const old=requests.at(-1);click();requests.at(-1).resolve();await flush();old.reject('https://secret.invalid/?X-Plex-Token=secret');await flush();assert.equal(alert(),'');
 click();const stale=requests.at(-1);h.props={...h.props,album:{...album,rating_key:'other'}};await ready();stale.reject('PLAYBACK_QUEUE_UNCERTAIN');await flush();assert.equal(alert(),'');
 click();const unmounted=requests.at(-1);h.slots.forEach(s=>s?.cleanup?.());const count=h.writes;unmounted.reject('private');await flush();assert.equal(h.writes,count);
});

test('real service admits before MPD IPC, carries the token, and leaves seek single-command',async()=>{
 const calls=[];let id=0;
 const service=compile(source('services/audio.ts'),{'../utils/playbackIntent':intents,'@tauri-apps/api/core':{invoke:async(cmd,args)=>{calls.push([cmd,args]);if(cmd==='admit_playback_intent')return String(++id);if(cmd==='playback_intent_current')return args.token===String(id)}},'../utils/frontendDiagnostics':{diagnosticInvoke:(invoke,cmd,args)=>invoke(cmd,args)}}).audioService;
 await service.playTracks([{title:'Fixture',uri:'fixture.flac'}],0);
 assert.deepEqual(calls.map(c=>c[0]),['admit_playback_intent','play_tracks']);assert.equal(calls[1][1].intent,'1');
 calls.length=0;await service.seek(12.5);assert.deepEqual(calls,[['seek_playback',{seconds:12.5}]]);reset();
});

test('initial-index admission refusal performs no MPD playback and retains its actionable category',async()=>{
 reset();const prior=intents.beginPlaybackIntent();prior.cancel();await flush();
 const calls=[];
 const service=compile(source('services/audio.ts'),{'../utils/playbackIntent':intents,'@tauri-apps/api/core':{invoke:async(cmd)=>{calls.push(cmd);if(cmd==='admit_playback_intent')throw 'FIRST_RUN_CATALOG_INDEXING';throw new Error('unexpected MPD IPC')}},'../utils/frontendDiagnostics':{diagnosticInvoke:(invoke,cmd,args)=>invoke(cmd,args)}}).audioService;
 for(const start of [()=>service.playTracks([track],0),()=>service.togglePlay(),()=>service.next(),()=>service.previous(),()=>service.playQueueIndex(0),()=>service.clearQueue()]){
  await assert.rejects(start(),e=>e==='FIRST_RUN_CATALOG_INDEXING');
 }
 assert.deepEqual(calls,Array(6).fill('admit_playback_intent'));
 assert.equal(intents.playbackFailure('FIRST_RUN_CATALOG_INDEXING'),'initialIndexing');
 assert.equal(intents.playbackFailure('CONFIG_SAVE_BUSY'),'failed');
 assert.equal(intents.playbackFailure('MANAGED_FOLDERS_SAVE_PENDING'),'failed');reset();
});
