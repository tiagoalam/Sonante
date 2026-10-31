import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import ts from 'typescript';
import i18next from 'i18next';
import { renderToStaticMarkup } from 'react-dom/server';
import * as intents from '../src/utils/playbackIntent.ts';
import * as metadata from '../src/utils/queueActions.ts';
import { createLocalSourceReadQueue } from '../src/utils/localSourceReadQueue.ts';
import { localAlbumForMedia } from '../src/utils/playerNavigation.ts';
import { flattenAlbumDiscs } from '../src/utils/localAlbumDiscs.ts';
const read = path => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
const compile = (path, deps, env = {}) => {
  const exports = {};
  const code = ts.transpileModule(read(path), { fileName: path, compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  new Function('require', 'exports', ...Object.keys(env), code)(id => { assert.ok(id in deps, id); return deps[id]; }, exports, ...Object.values(env));
  return exports;
};
const flush = async () => { for (let i = 0; i < 70; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const nodes = (tree, pred) => Array.isArray(tree) ? tree.flatMap(n => nodes(n, pred)) : !tree || typeof tree !== 'object' ? [] : [...(pred(tree) ? [tree] : []), ...nodes(tree.props?.children, pred)];
const local = (title = 'Synthetic track', path = 'Source/Album/track.flac') => ({ title, artist: 'Synthetic artist', album: 'Synthetic album', media_locator: { kind: 'local', uri: path }, duration: 182 });
const eq = (a,b) => a?.length === b?.length && a.every((v,i) => Object.is(v,b[i]));

let generation = 0;
async function fixture(language = 'en') {
  let current;
  const calls = [], errors = [], events = new Map(), doc = { body: {}, activeElement: null };
  const i18n = i18next.createInstance(); await i18n.init({ lng: language, resources: { [language]: { translation: JSON.parse(read(`locales/${language}.json`)) } } });
  intents.configurePlaybackIntents(async () => String(++generation), async token => { if (String(generation) === token) generation++; }, async token => String(generation) === token);
  const hooks = {
    useState(value) { const host = current, n = host.cursor++; const slot = host.slots[n] ??= { value: typeof value === 'function' ? value() : value }; return [slot.value, next => { assert.ok(!host.disposed, 'no post-unmount writes'); slot.value = typeof next === 'function' ? next(slot.value) : next; }]; },
    useRef(value) { return current.slots[current.cursor++] ??= { current: value }; },
    useMemo(fn,deps) { const host = current,n=host.cursor++; if (!eq(host.slots[n]?.deps,deps))host.slots[n]={deps,value:fn()};return host.slots[n].value; },
    useEffect(fn,deps) { const host=current,n=host.cursor++,old=host.slots[n];if(!eq(old?.deps,deps))host.effects.push(()=>{old?.cleanup?.();host.slots[n]={deps,cleanup:fn()};}); },
  };
  hooks.useLayoutEffect = hooks.useEffect;
  class Harness {
    cursor=0; slots=[]; effects=[]; disposed=false;
    constructor(render) { this.render=render; }
    paint(commit=true) { current=this; this.cursor=0;this.effects=[];this.tree=this.render();current=null;if(commit)this.commit();return this.tree; }
    commit() { this.effects.splice(0).forEach(fn=>fn()); }
    close() { this.slots.forEach(slot=>slot?.cleanup?.());this.disposed=true; }
  }
  const win = { innerWidth: 600, innerHeight: 400, addEventListener: (name,fn) => events.set(name,fn), removeEventListener: name => events.delete(name) };
  const audioService = { enqueueTracks: async (...args) => { calls.push(args); return args[0].length; } };
  const { useQueueInsertion } = compile('components/QueueInsertionMenu.tsx', {
    react: { ...React,...hooks }, 'react/jsx-runtime': jsx, 'react-dom': { createPortal: tree => tree }, 'lucide-react': icons,
    'react-i18next': { useTranslation: () => ({ t:i18n.t.bind(i18n) }) }, '../utils/playbackIntent': intents,
    '../services/audio': { audioService }, '../utils/frontendDiagnostics': { logError: (...args) => errors.push(args) },
  }, { window: win, document: doc });
  const f = { context:{},available:true,calls,errors,events,doc,audioService,Harness,react:{...React,...hooks},useQueueInsertion,t:i18n.t.bind(i18n) };
  f.host = new Harness(()=>useQueueInsertion(f.context,f.available));f.paint=()=>f.host.paint();f.paint();
  f.origin={isConnected:true,focused:0,focus(){this.focused++;},querySelector(){return null;},getBoundingClientRect:()=>({left:20,bottom:40})};
  f.event = (x=0,y=0)=>({currentTarget:f.origin,clientX:x,clientY:y,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;}});
  f.open = request => { const event=f.event();f.host.tree.open(event,request);f.paint();assert.equal(event.prevented,true);assert.equal(event.stopped,true);return f.host.tree.menu; };
  f.choose = async action => { const menu=f.host.tree.menu;assert.ok(menu);menu.props.choose(action);await flush();return f.paint(); };
  return f;
}

for (const language of ['en','pt-BR']) test(`queue menu opens without effects, supports all actions and reports only confirmed counts (${language})`,async()=>{
  const f=await fixture(language);let albums=0;const track=local(),full=[track,local('Second disc','Source/Disc2/2.flac')];
  const request={track,album:async()=>{albums++;return full;}};
  const trigger=f.host.tree.trigger(request),event=f.event();trigger.props.onClick(event);f.paint();assert.equal(event.stopped,true);assert.deepEqual(f.calls,[]);assert.equal(albums,0);
  let tree=await f.choose('next');assert.equal(f.calls.length,1);assert.deepEqual(f.calls[0].slice(0,2),[[track],'next']);assert.equal(tree.notice.props.role,'status');assert.ok(renderToStaticMarkup(tree.notice).includes(f.t('queueActions.added',{count:1})));
  f.open(request);await f.choose('end');assert.equal(f.calls[1][1],'end');assert.equal(albums,0);
  f.open(request);tree=await f.choose('album');assert.equal(albums,1);assert.deepEqual(f.calls[2].slice(0,2),[full,'end']);assert.ok(renderToStaticMarkup(tree.notice).includes(f.t('queueActions.added',{count:2})));
  assert.deepEqual(f.errors,[]);f.host.close();
});

test('queue action blocks double submission and drops preparation after navigation, availability loss, unmount or a newer playback intent',async()=>{
  for(const mode of ['double','navigation','availability','unmount','newer','request']) {
    const f=await fixture(),slow=deferred();let current=true,reads=0;
    f.open({track:local(),album:()=>{reads++;return slow.promise;},current:()=>current});
    const menu=f.host.tree.menu;menu.props.choose('album');menu.props.choose('album');await flush();assert.equal(reads,1);assert.equal(f.calls.length,0);
    if(mode==='navigation'){f.context={};f.paint();}
    if(mode==='availability'){f.available=false;f.paint();}
    if(mode==='unmount')f.host.close();
    if(mode==='newer')intents.beginPlaybackIntent();
    if(mode==='request')current=false;
    slow.resolve([local()]);await flush();
    assert.equal(f.calls.length,mode==='double'?1:0,mode);
    if(mode!=='unmount'){const result=f.paint();assert.equal(Boolean(result.notice),mode==='double',mode);f.host.close();}
  }
});

test('late success and failure cannot write feedback after cancellation; unknown album has no effect',async()=>{
  for(const fail of [false,true]){
    const f=await fixture(),pending=deferred();f.audioService.enqueueTracks=async(...args)=>{f.calls.push(args);return pending.promise;};
    f.open({track:local()});const menu=f.host.tree.menu;menu.props.choose('album');await flush();assert.equal(f.calls.length,0);menu.props.choose('end');await flush();assert.equal(f.calls.length,1);
    f.host.close();if(fail)pending.reject('private failure');else pending.resolve(1);await flush();assert.deepEqual(f.errors,[]);
  }
});

test('failures are translated, distinguish uncertain/persistence/receiver outcomes, never retry and hide private diagnostics',async()=>{
  for(const error of ['PLAYBACK_QUEUE_UNCERTAIN','PLAYBACK_PERSISTENCE_FAILED','QUEUE_INSERT_UPNP_ACTIVE','ACK [50@1] /private/source?token=SECRET']){
    const f=await fixture('pt-BR');f.audioService.enqueueTracks=async(...args)=>{f.calls.push(args);throw error;};f.open({track:local()});const result=await f.choose('end');
    assert.equal(f.calls.length,1);assert.equal(result.notice.props.role,'alert');const html=renderToStaticMarkup(result.notice);assert.doesNotMatch(html,/SECRET|ACK|\/private|queueActions\./);
    const key=error==='PLAYBACK_QUEUE_UNCERTAIN'?'uncertain':error==='PLAYBACK_PERSISTENCE_FAILED'?'persistence':error==='QUEUE_INSERT_UPNP_ACTIVE'?'receiver':'failed';assert.ok(html.includes(f.t(`queueActions.${key}`)));assert.equal(f.errors.length,1);f.host.close();
  }
});

test('unavailable/stale trigger performs no admission and the menu restores focus, clamps bounds, navigates keys and closes without playback',async()=>{
  const f=await fixture();f.available=false;f.paint();const request={track:local()};assert.equal(f.host.tree.trigger(request).props.disabled,true);assert.equal(f.open(request),null);
  f.available=true;f.paint();assert.equal(f.open({...request,current:()=>false}),null);f.open(request);
  const entry=f.host.tree.menu;const menuHost=new f.Harness(()=>entry.type(entry.props));let menu=menuHost.paint(false);
  const panel=nodes(menu,n=>n.props?.role==='menu')[0],buttons=nodes(panel,n=>n.type==='button');assert.equal(buttons.length,3);assert.equal(buttons[2].props.disabled,true);
  const domButtons=buttons.filter(n=>!n.props.disabled).map(()=>({focus(){f.doc.activeElement=this;}}));
  panel.props.ref.current={getBoundingClientRect:()=>({width:280,height:160}),querySelector:()=>domButtons[0],querySelectorAll:()=>domButtons,contains:()=>false};menuHost.commit();assert.equal(f.doc.activeElement,domButtons[0]);
  let stopped=0,prevented=0;const key=key=>panel.props.onKeyDown({key,currentTarget:panel.props.ref.current,preventDefault(){prevented++;},stopPropagation(){stopped++;}});
  key('ArrowDown');assert.equal(f.doc.activeElement,domButtons[1]);key('ArrowDown');assert.equal(f.doc.activeElement,domButtons[0]);key('End');assert.equal(f.doc.activeElement,domButtons[1]);key('Home');assert.equal(f.doc.activeElement,domButtons[0]);
  key('Escape');assert.equal(stopped,5);assert.equal(prevented,5);assert.equal(f.paint().menu,null);menuHost.close();assert.equal(f.origin.focused,1);assert.equal(f.events.size,0);
  f.open(request);const second=f.host.tree.menu,child=new f.Harness(()=>second.type(second.props));menu=child.paint();const backdrop=menu;const e={target:{},currentTarget:{},stopPropagation(){this.stopped=true;},preventDefault(){this.prevented=true;}};e.target=e.currentTarget;backdrop.props.onMouseDown(e);assert.equal(e.stopped,true);assert.equal(e.prevented,true);assert.equal(f.paint().menu,null);assert.equal(f.calls.length,0);child.close();f.host.close();
});

test('enqueue service passes the explicit token once, bypasses gain/playback preparation and rejects canceled work before IPC',async()=>{
  const calls=[],queue=createLocalSourceReadQueue();let backend=0;intents.configurePlaybackIntents(async()=>String(++backend),async()=>{});
  const {audioService}=compile('services/audio.ts',{
    './config':{ensureDirectOutputChoice:async()=>assert.fail('queue editing cannot authorize gain')},
    '../utils/localCoverBatch':{createLocalCoverBatch:()=>()=>assert.fail('no artwork read')},
    '../utils/localSourceReadQueue':{localSourceReadQueue:queue},'../utils/playbackIntent':{...intents,configurePlaybackIntents(){}},
    '@tauri-apps/api/core':{invoke:async(cmd,args)=>{calls.push([cmd,args]);return 1;}},'../utils/frontendDiagnostics':{diagnosticInvoke:(invoke,...args)=>invoke(...args)},
  });
  const intent=intents.beginPlaybackIntent();assert.equal(await audioService.enqueueTracks([local()],'next',intent),1);assert.deepEqual(calls,[['enqueue_tracks',{tracks:[local()],placement:'next',intent:'1'}]]);
  const slow=deferred();void queue(()=>slow.promise);await flush();const obsolete=intents.beginPlaybackIntent();const pending=audioService.enqueueTracks([local()],'end',obsolete);await flush();intents.beginPlaybackIntent();slow.resolve();await assert.rejects(pending,e=>e==='PLAYBACK_SUPERSEDED');assert.equal(calls.length,1);
});

test('metadata and whole-album loading preserve exact source identities, CUE/disc order and Plex occurrence; ambiguous names are never guessed',async()=>{
  const cue={name:'Movement',path:'Source/Disc1/image.cue/track0001',duration:120},album={id:'a',title:'Synthetic album',artist:'Synthetic artist',folder_path:'Source',discs:[{folder_path:'Source/Disc1/image.cue'}]};
  assert.deepEqual(metadata.localQueueTrack(cue,album).media_locator,{kind:'local',uri:cue.path});
  const plex={title:'Plex track',duration_ms:182000,artist:'Track artist',media_locator:{kind:'plex',server_id:'server',part_key:'/library/parts/1'}};
  assert.equal(metadata.plexQueueTrack(plex).duration,182);assert.equal(metadata.plexQueueTrack(plex).artist,'Track artist');
  const calls=[],snapshot={loaded:true,albums:[album]},disc2={name:'Disc2',path:'Source/Disc2/2.flac'};
  const {loadQueuedAlbum}=compile('utils/queueAlbumLoader.ts',{
    '../services/audio':{audioService:{getLocalAlbums:async()=>{calls.push('index');return snapshot.albums;},getLocalAlbumTracks:async id=>{calls.push(['tracks',id]);return [{disc:{number:1},tracks:[cue]},{disc:{number:2},tracks:[disc2]}];}}},
    '../services/plex':{plexService:{resolvePlayerNavigation:async media=>{calls.push(['resolve',media]);return {navigation:{album:{rating_key:'plex-album'}}};},getAlbumTracks:async id=>{calls.push(['plex-tracks',id]);return [plex];}}},
    './localAlbumCatalogSession':{localAlbumCatalog:{getSnapshot:()=>snapshot}},'./playerNavigation':{localAlbumForMedia},'./localAlbumDiscs':{flattenAlbumDiscs},'./queueActions':metadata,
  });
  const tracks=await loadQueuedAlbum(metadata.localQueueTrack(cue,album));assert.deepEqual(tracks.map(t=>t.media_locator.uri),[cue.path,disc2.path]);assert.deepEqual(calls,[['tracks','a']]);
  snapshot.albums=[album,{...album,id:'ambiguous'}];await assert.rejects(loadQueuedAlbum(metadata.localQueueTrack(cue)),e=>e==='QUEUE_INSERT_ALBUM_UNAVAILABLE');assert.equal(calls.length,1);
  await loadQueuedAlbum(metadata.plexQueueTrack(plex),7);assert.equal(calls[1][1].queue_index,7);assert.deepEqual(calls[2],['plex-tracks','plex-album']);
  await assert.rejects(loadQueuedAlbum({...local(),media_locator:{kind:'upnp',session_id:'owned'}}),e=>e==='QUEUE_INSERT_ALBUM_UNAVAILABLE');
});

test('real Plex album context menu and ellipsis insert exact ordered album references without invoking row playback',async()=>{
 const f=await fixture();f.host.close();
 const tracks=[{title:'Disc 1',rating_key:'1',duration_ms:182000,media_locator:{kind:'plex',server_id:'server',part_key:'/library/parts/1'}},{title:'Disc 2',rating_key:'2',duration_ms:1000,media_locator:{kind:'plex',server_id:'server',part_key:'/library/parts/2'}}];
 let plays=0;
 const {AlbumView}=compile('components/AlbumView.tsx',{
  react:f.react,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:f.t})},'lucide-react':icons,
  './QueueInsertionMenu':{useQueueInsertion:f.useQueueInsertion},'../utils/queueActions':metadata,
  '../utils/usePlaybackAttempt':{usePlaybackAttempt:()=>({failure:null,run:async()=>{plays++;}})},'./PlaybackRequestNotice':{PlaybackRequestNotice:()=>null},
  '../utils/frontendDiagnostics':{logError:()=>{}},'../services/plex':{plexService:{getAlbumTracks:async()=>tracks}},
  '../services/audio':{audioService:f.audioService},'../services/favorites':{favoritesService:{getFavorites:async()=>[]}},
  './PlexImage':{PlexImage:()=>null},'./PlexFileInfoModal':{PlexFileInfoModal:()=>null},'./PlaylistPickerModal':{PlaylistPickerModal:()=>null},
 });
 const props={album:{rating_key:'album',server_id:'server',title:'Synthetic',artist:'Artist'},active:true,isPlaybackAvailable:true,onBack(){}};
 const host=new f.Harness(()=>AlbumView(props));host.paint();await flush();let tree=host.paint();const row=nodes(tree,n=>n.props?.onContextMenu)[0];assert.ok(row);row.props.onContextMenu(f.event());tree=host.paint();
 let menu=nodes(tree,n=>n.props?.choose&&n.props?.target)[0];assert.ok(menu);menu.props.choose('album');await flush();tree=host.paint();
 assert.equal(plays,0);assert.equal(f.calls.length,1);assert.deepEqual(f.calls[0][0].map(track=>track.media_locator),tracks.map(track=>track.media_locator));assert.equal(f.calls[0][1],'end');
 const trigger=nodes(tree,n=>n.props?.['data-queue-trigger'])[1];trigger.props.onClick(f.event());tree=host.paint();menu=nodes(tree,n=>n.props?.choose&&n.props?.target)[0];menu.props.choose('next');await flush();host.paint();
 assert.equal(f.calls.length,2);assert.deepEqual(f.calls[1][0][0].media_locator,tracks[1].media_locator);assert.equal(f.calls[1][1],'next');assert.equal(plays,0);host.close();
});
