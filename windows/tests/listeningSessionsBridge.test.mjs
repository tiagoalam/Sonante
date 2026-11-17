import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import ts from 'typescript';
import {loadConfigFromFile} from 'vite';
const root=fileURLToPath(new URL('../../',import.meta.url));
const account='a'.repeat(64), connection='synthetic-connection';
const sources=[{kind:'local'},{kind:'plex',server_id:'synthetic-server',library_id:'1'},{kind:'navidrome',account_id:account}];
const candidate={id:'fixture-album',title:'Fixture Album',artist:'Synthetic Artist',year:2000,genres:['Jazz'],added_at:null};
const locators=source=>[1,2].map(n=>source.kind==='local'?{kind:'local',uri:`windows/fixture-album/1/track-${n}`}:source.kind==='plex'?{kind:'plex',server_id:source.server_id,part_key:`/library/parts/${n}/file.flac`,rating_key:String(n)}:{kind:'navidrome',account_id:account,connection_id:connection,song_id:String(n)});
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve}};
function fixture(source,options={}) {
 const modules=new Map(),values=new Map(),calls=[];
 const storage={getItem:k=>values.get(k)??null,setItem:(k,v)=>{calls.push({name:'storage.write'});values.set(k,v)}};
 const browser={localStorage:storage,dispatchEvent:()=>true,confirm:()=>assert.fail('Shared must not ask for Direct gain')};
 let queue=[],revision='initial',index=0,serial=0,admission=0,activeToken=null,navReads=0;
 const prefs={usage_mode:'full',first_run:false,experimental_features:true,roots:['C:/SyntheticFixture'],sources:{local:true,navidrome:true,plex:true},plex:{url:'',server_id:'synthetic-server',server_name:'Synthetic',saved:true,revision:'1'}};
 const status=()=>({observation:String(++serial),generation:1,engineRunning:queue.length>0,state:queue.length?'play':'idle',elapsed:0,duration:900,volume:50,audio:'',warning:null,queue,queueIndex:index,queueRevision:revision});
 const native=async(name,args={})=>{
  calls.push({name,args});
  if(name==='windows_preferences')return structuredClone(prefs);
  if(name==='windows_smart_playlists'){
   if(args.operation==='sources')return {local:true,plex_server_id:'synthetic-server',navidrome_account_id:account};
   if(args.operation==='catalog')return [candidate];
  }
  if(name==='windows_catalog'){if(options.catalog)await options.catalog;return {albums:[{...candidate,year:'2000',source_id:'own-root',source_label:'Synthetic'}]};}
  if(name==='windows_catalog_tracks')return [{disc:{number:1},tracks:locators(source).map((l,n)=>({item_type:'file',name:`Track ${n+1}`,path:l.uri,duration:900}))}];
  if(name==='get_album_tracks')return locators(source).map((l,n)=>({title:`Track ${n+1}`,media_locator:l,duration_ms:900000}));
  if(name==='navidrome_status')return {connection_id:options.swapAccount&&++navReads>1?'replacement':connection,account_id:account};
  if(name==='navidrome_album')return {songs:[2,1].map(n=>({id:String(n),title:`Track ${n}`,artist:candidate.artist,album:candidate.title,disc:1,track:n,duration:900}))};
  if(name==='windows_intent'){
   if(args.operation==='admit')return activeToken=`synthetic-${++admission}`;
   if(args.operation==='current')return args.token===activeToken;
   if(args.operation==='cancel'){if(activeToken===args.token)activeToken=null;return;}
  }
  if(name==='windows_play_tracks'||name==='windows_play_selection'){
   if(options.failPlay)throw 'windows_playback_unconfirmed';
   assert.equal(args.intent,activeToken,'same admitted explicit gesture reaches native');
   const tracks=args.tracks??args.uris.map(uri=>({media_locator:{kind:'local',uri}}));
   queue=tracks.map((t,n)=>({id:`track-${n+1}`,title:t.title??`Track ${n+1}`,artist:candidate.artist,album:candidate.title,disc:1,fileName:`${n+1}.flac`,catalogAlbumId:source.kind==='local'?candidate.id:null,remoteSource:source.kind==='local'?null:{...t.media_locator},remoteConnectionId:source.kind==='navidrome'?connection:null}));
   index=args.startIndex;revision='accepted';return status();
  }
  if(name==='windows_status')return status();
  assert.fail(`Unexpected native operation: ${name}`);
 };
 let bridge;
 function load(file){
  file=path.resolve(root,file);if(modules.has(file))return modules.get(file);
  const exports={};modules.set(file,exports);
  // The release build replaces Vite's DEV environment. Keep diagnostics real,
  // but substitute that compile-time value in this isolated CommonJS fixture.
  const sourceText=fs.readFileSync(file,'utf8').replaceAll('import.meta.env','({DEV:false})');
  const code=ts.transpileModule(sourceText,{fileName:file,compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS}}).outputText;
  new Function('require','exports','window',code)(name=>{
   if(name==='@tauri-apps/api/core'||file.endsWith('/windows/src/smartPlaylists.ts')&&name==='./core')return{invoke:(...args)=>bridge(...args)};
   if(name==='i18next')return{default:{t:x=>x}};
   assert.ok(name.startsWith('.'),'Unexpected external dependency '+name);
   let target=path.resolve(path.dirname(file),name).replace(/\.tsx?$/,'');
   // Same service alias as the actual Windows Vite resolver. The wrapper's
   // absolute original import is deliberately not redirected to itself.
   if(file.startsWith(path.join(root,'src/'))&&target===path.join(root,'src/services/smartPlaylists'))target=path.join(root,'windows/src/smartPlaylists');
   return load(target+'.ts');
  },exports,browser);return exports;
 }
 bridge=load('windows/src/compat.ts').createWindowsBridge(native,()=>storage);
 const service=load('src/services/listeningSessions.ts'),audio=load('src/services/audio.ts').audioService;
 const helpers=load('src/utils/listeningSessions.ts'),store=load('src/utils/listeningSessionStore.ts').listeningSessionStore;
 const progress=load('src/utils/listeningSessionProgress.ts');
 const policy=load('src/utils/experimentalFeatures.ts');
 return{service,audio,helpers,store,progress,policy,bridge,calls,values,status,source,changeIndex:n=>index=n,changeRevision:()=>revision='edited',
  async prepare(){const catalog=await service.sessionCatalog(source,()=>true);const selected=await helpers.prepareSessionAlbums(catalog,source,1800,'discovery',a=>service.sessionAlbumTracks(source,a,()=>true),()=>true);return{session:{id:'synthetic-session',name:'Own Fixture',created_at:1,source,target:1800,albums:selected.albums,cursor:0,completed:false},tracks:selected.tracks.flat()};},
  async play(prepared){await audio.playTracks(prepared.tracks,prepared.session.cursor);await progress.bindSessionProgress(prepared.session,()=>true);}
 };
}
for(const source of sources)test(`${source.kind}: original session catalog, complete album, explicit save/play, native queue projection and saved progress`,async()=>{
 const f=fixture(source);assert.equal(f.calls.length,0,'loading modules starts no engine, listener or reads');
 const p=await f.prepare();assert.equal(p.tracks.length,2);assert.deepEqual(p.tracks.map(t=>f.helpers.sessionTrackKey(t.media_locator)),locators(source).map(f.helpers.sessionTrackKey));
 assert.equal(f.values.size,0,'preparation is a draft');assert.equal(f.calls.filter(c=>c.name.startsWith('windows_play')).length,0);
 f.store.save(p.session);await f.play(p);assert.equal(f.progress.getSessionPlayback().id,p.session.id);
 f.changeIndex(1);await f.progress.observeSessionProgress(await f.audio.getStatus());assert.equal(f.store.list()[0].cursor,1);
 const persisted=f.values.get(f.helpers.SESSION_KEY);assert.equal(f.helpers.readSessions({getItem:()=>persisted})[0].cursor,1,'fresh reader restores saved cursor');
 assert.equal(f.calls.filter(c=>c.name=== (source.kind==='local'?'windows_play_selection':'windows_play_tracks')).length,1);
 assert.equal(f.calls.filter(c=>c.name==='windows_action'||c.name==='windows_gain').length,0,'progress never alters transport or gain');
});
test('late Local catalog cancellation cannot publish a draft, write or begin playback',async()=>{
 const delayed=deferred(),f=fixture(sources[0],{catalog:delayed.promise});let live=true;
 const read=f.service.sessionCatalog(f.source,()=>live);await new Promise(r=>setImmediate(r));live=false;delayed.resolve();
 await assert.rejects(read,e=>e==='SESSION_CANCELLED');assert.equal(f.values.size,0);assert.equal(f.calls.some(c=>c.name.startsWith('windows_play')),false);
});
test('Navidrome reconnection between album reads refuses the entire session before play',async()=>{
 const f=fixture(sources[2],{swapAccount:true});await assert.rejects(f.prepare(),e=>e==='SESSION_SOURCE');assert.equal(f.values.size,0);assert.equal(f.calls.some(c=>c.name.startsWith('windows_play')),false);
});
test('native playback refusal preserves the explicit save and never binds progress or retries',async()=>{
 const f=fixture(sources[1],{failPlay:true}),p=await f.prepare();f.store.save(p.session);const old=f.values.get(f.helpers.SESSION_KEY);
 await assert.rejects(f.play(p),e=>e==='windows_playback_unconfirmed');assert.equal(f.progress.getSessionPlayback(),null);assert.equal(f.values.get(f.helpers.SESSION_KEY),old);
 assert.equal(f.calls.filter(c=>c.name==='windows_play_tracks').length,1);assert.equal(f.calls.filter(c=>c.name==='windows_status').length,0);
});
test('changed native queue revision suspends tracking without overwriting saved cursor or touching audio',async()=>{
 const f=fixture(sources[0]),p=await f.prepare();f.store.save(p.session);await f.play(p);const old=f.values.get(f.helpers.SESSION_KEY),before=f.calls.length;
 f.changeIndex(1);f.changeRevision();await f.progress.observeSessionProgress(await f.audio.getStatus());assert.equal(f.progress.getSessionPlayback(),null);assert.equal(f.values.get(f.helpers.SESSION_KEY),old);
 assert.ok(f.calls.slice(before).every(c=>c.name==='windows_status'));
});
test('original Full/Psico observer suspends progress in Simple without deleting sessions or changing playback',async()=>{
 const f=fixture(sources[0]),p=await f.prepare();f.store.save(p.session);await f.play(p);const old=f.values.get(f.helpers.SESSION_KEY);
 const observer=f.policy.createExperimentalSessionObserver(async()=>f.progress);
 assert.equal(f.policy.experimentalFeaturesEnabled({experimental_features:true,usage_mode:'full'}),true);
 await observer.update(true,await f.audio.getStatus());const before=f.calls.length;
 assert.equal(f.policy.experimentalFeaturesEnabled({experimental_features:true,usage_mode:'simple'}),false);
 await observer.update(false,await f.audio.getStatus());assert.equal(f.progress.getSessionPlayback(),null);assert.equal(f.values.get(f.helpers.SESSION_KEY),old);
 assert.ok(f.calls.slice(before).every(c=>c.name==='windows_status'));observer.dispose();
});
test('actual Windows Vite transform retains original session UI and observer while retaining Plex account adaptation',async()=>{
 const result=await loadConfigFromFile({command:'build',mode:'production'},path.join(root,'windows/vite.config.ts'));
 const plugin=result.config.plugins.find(p=>p.name==='sonante-windows-capabilities');const file=path.join(root,'src/App.tsx');
 const transformed=plugin.transform(fs.readFileSync(file,'utf8'),file).code;
 for(const marker of ['sessionProgress.update(experimental, playbackStatus)','{experimental && isExploreSection &&','experimental && centralSection === "sessions"','if (!config || experimental) return;'])assert.ok(transformed.includes(marker),marker);
 assert.ok(transformed.includes('plexAccountIdentity(config)'));
});
