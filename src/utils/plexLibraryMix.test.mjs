import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import * as explorePresentation from './explorePresentation.ts';
import * as presentation from './plexDiscoverPresentation.ts';
import * as mixTools from './plexLibraryMix.ts';
import { PlexDiscoverSession } from './plexDiscover.ts';
import { balancedMix, mixTrackKey, mixPlaybackMetadata, mixPlaylistItems } from './plexLibraryMix.ts';
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<250;i++)await Promise.resolve();};
const scope=(keys=['1'],libraryId=keys.length===1?keys[0]:'all')=>({identity:{},serverId:'server',libraryId,language:'en',libraries:keys.map(key=>({key,title:'Library '+key}))});
const track=(id,library_id='1',server_id='server')=>({server_id,library_id,rating_key:String(id),title:'Homonym',artist:'Artist',album_title:'Album',duration_ms:120000,thumb:null,media_locator:{kind:'plex',server_id,rating_key:String(id),part_key:`/library/parts/${id}/file.flac`}});
const reply=(s,id,tracks,has_more=false,partial=false)=>({server_id:s.serverId,library_id:id,language:s.language,tracks,has_more,partial});
const create=(s,query,load=async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}))=>{
 const p=new PlexDiscoverSession(load,async()=>assert.fail('no album/detail requests'),async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),()=>0,undefined,query);p.setContext(s);p.ensure();return p;
};
// Compile both actual modules: diagnostics gating/serialization must not be mocked away.
function diagnosticSession(env, info) {
 const helperSource=readFileSync(new URL('./frontendDiagnostics.ts',import.meta.url),'utf8').replaceAll('import.meta.env',JSON.stringify(env));
 const options={compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}};
 const helper={};new Function('exports','console',ts.transpileModule(helperSource,options).outputText)(helper,{info});
 const source=readFileSync(new URL('./plexDiscover.ts',import.meta.url),'utf8'),exports={};
 const deps={'./explorePresentation.ts':explorePresentation,'./plexDiscoverPresentation.ts':presentation,'./plexLibraryMix.ts':mixTools,'./frontendDiagnostics.ts':helper};
 new Function('require','exports',ts.transpileModule(source,options).outputText)(name=>{assert.ok(name in deps,name);return deps[name];},exports);
 return (s,query)=>{
  const p=new exports.PlexDiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),()=>0,undefined,query);
  p.setContext(s);p.ensure();return p;
 };
}
test('mix is lazy, finite, stable through render/navigation/refresh, and regenerates only explicitly',async()=>{
 const s=scope(),calls=[];const p=create(s,async(server,id,language,limit)=>{calls.push({server,id,language,limit});return reply(s,id,Array.from({length:100},(_,i)=>track(i+calls.length*100)));});await flush();assert.equal(calls.length,0);
 p.openMix();await flush();assert.equal(calls.length,1);assert.equal(calls[0].limit,100);assert.equal(p.getSnapshot().mix.tracks.length,100);const selection=p.getSnapshot().mix.tracks;
 for(let i=0;i<3;i++){p.getSnapshot();p.ensure();p.openMix();p.back();p.setContext({...s});}p.retry();await flush();assert.equal(p.getSnapshot().mix.tracks,selection);assert.equal(calls.length,1);
 p.generateMix();p.generateMix();await flush();assert.equal(calls.length,2);assert.notDeepEqual(p.getSnapshot().mix.tracks,selection);
});
test('All waits for each library then balances regardless of arrival order and retains source homonyms',async()=>{
 const s=scope(['1','2','3']),pending=[];const p=create(s,(server,id,language,limit)=>{const d=deferred();pending.push({id,limit,d});return d.promise;});await flush();p.openMix();assert.equal(pending.length,2);
 pending[1].d.resolve(reply(s,pending[1].id,Array.from({length:34},(_,i)=>track(i,pending[1].id))));await flush();assert.equal(p.getSnapshot().mix.phase,'loading');assert.equal(p.getSnapshot().mix.tracks.length,0);assert.equal(pending.length,3);
 for(const request of [pending[0],pending[2]])request.d.resolve(reply(s,request.id,Array.from({length:34},(_,i)=>track(i,request.id))));await flush();
 const mix=p.getSnapshot().mix;assert.equal(mix.tracks.length,100);const counts=s.libraries.map(l=>mix.tracks.filter(t=>t.library_id===l.key).length);assert.ok(Math.max(...counts)-Math.min(...counts)<=1);assert.equal(new Set(mix.tracks.map(mixTrackKey)).size,100);assert.ok(mix.tracks.every(t=>t.library_title==='Library '+t.library_id));
});
test('small/failed libraries redistribute vacancies once with bounded queries and disclose partial results',async()=>{
 const s=scope(['1','2','3']),calls=[];const p=create(s,async(server,id,lang,limit)=>{calls.push({id,limit});if(id==='3')throw 'PLEX_DISCOVER_AUTH';if(id==='1')return reply(s,id,[track(1,id)]);return reply(s,id,Array.from({length:limit},(_,i)=>track(i,id)),true);});await flush();p.openMix();await flush();
 assert.equal(p.getSnapshot().mix.tracks.length,100);assert.equal(calls.filter(c=>c.id==='2').length,2);assert.equal(calls.length,4);assert.ok(calls.every(c=>c.limit<=100));assert.equal(p.getSnapshot().mix.partial,true);assert.deepEqual(p.getSnapshot().mix.issues,[{library:'Library 3',error:'PLEX_DISCOVER_AUTH'}]);
});
test('duplicates use track identity, fewer than100 is honest, failed refill preserves first-round candidates',async()=>{
 const s=scope(),calls=[];const p=create(s,async(server,id,lang,limit)=>{calls.push(limit);if(calls.length>1)throw 'PLEX_DISCOVER_UNAVAILABLE';return reply(s,id,[track(1),track(1),track(2)],true);});await flush();p.openMix();await flush();assert.equal(calls.length,2);assert.equal(p.getSnapshot().mix.tracks.length,2);assert.equal(p.getSnapshot().mix.partial,true);assert.equal(p.getSnapshot().mix.limited,true);
 assert.equal(balancedMix([[track(1),track(1)],[track(1,'2')]]).length,2);const metadata=mixPlaybackMetadata(p.getSnapshot().mix.tracks);assert.equal(metadata[0].artist,'Artist');assert.equal(metadata[0].duration,120);assert.equal(metadata[0].media_locator,p.getSnapshot().mix.tracks[0].media_locator);assert.ok(metadata.every(t=>!('uri' in t)));
});
test('new server/account/library/language drops obsolete replies while actual queries keep shared capacity',async()=>{
 for(const change of [{identity:{}},{serverId:'new-server'},{libraryId:'2',libraries:[{key:'2',title:'New'}]},{language:'pt-BR'}]){
  const s=scope(['1','2']),pending=[];let active=0,max=0;const p=create(s,(server,id,lang,limit)=>{active++;max=Math.max(max,active);const d=deferred();pending.push({server,id,lang,d});return d.promise.finally(()=>active--);});await flush();p.openMix();const changed={...s,...change};p.setContext(changed);assert.equal(p.getSnapshot().mix.opened,false);p.openMix();assert.equal(pending.length,2);
  for(const request of pending.slice(0,2))request.d.resolve(reply(s,request.id,[track(1,request.id)]));await flush();assert.equal(p.getSnapshot().mix.tracks.length,0);assert.equal(max,2);
  for(const request of pending.slice(2))request.d.resolve(reply(changed,request.id,[track(8,request.id,request.server)]));await flush();assert.ok(p.getSnapshot().mix.tracks.every(t=>t.server_id===changed.serverId));assert.equal(max,2);
 }
});
test('membership, locator identity and response size fail visibly without exposing foreign candidates',async()=>{
 const s=scope();for(const result of [reply(s,'foreign',[track(1)]),reply(s,'1',[track(1,'other')]),reply(s,'1',[{...track(1),media_locator:{...track(1).media_locator,server_id:'other'}}]),reply(s,'1',Array.from({length:101},(_,i)=>track(i)))]){
  const p=create(s,async()=>result);await flush();p.openMix();await flush();assert.equal(p.getSnapshot().mix.phase,'error');assert.equal(p.getSnapshot().mix.tracks.length,0);assert.equal(p.getSnapshot().mix.issues.length,1);
 }
});
test('total failure has no partial-preview claim while successful partial previews remain explicit',async()=>{
 const s=scope(),p=create(s,async()=>{throw 'PLEX_DISCOVER_INVALID';});await flush();p.openMix();await flush();
 const mix=p.getSnapshot().mix;assert.equal(mix.phase,'error');assert.equal(mix.tracks.length,0);assert.equal(mix.partial,false);assert.equal(mix.limited,false);
 let plays=0;await p.playMix(async()=>plays++);assert.equal(plays,0);
});
test('DEV failures identify IPC versus frontend validation with bounded counts and no private data',async()=>{
 const messages=[],make=diagnosticSession({DEV:true},(prefix,payload)=>messages.push({prefix,...JSON.parse(payload)}));
 const s=scope();
 for(const [query,stage,category,reason,observed] of [
  [async()=>{throw new Error('https://private?X-Plex-Token=secret /home/private response');},'ipc','PLEX_DISCOVER_UNAVAILABLE','operation_rejected',0],
  [async()=>({...reply(s,'1',[]),server_id:'foreign'}),'frontend','PLEX_DISCOVER_CONTEXT','response_scope',0],
  [async()=>reply(s,'1',null),'frontend','PLEX_DISCOVER_INVALID','response_shape',0],
  [async()=>reply(s,'1',[track(1,'foreign')]),'frontend','PLEX_DISCOVER_CONTEXT','track_or_locator_scope',1],
  [async()=>reply(s,'1',Array.from({length:999},(_,i)=>track(i))),'frontend','PLEX_DISCOVER_INVALID','response_shape',101],
 ]){
  const p=make(s,query);await flush();p.openMix();await flush();
  assert.deepEqual(messages.at(-1),{prefix:'[Sonante] plex.mix.frontend',stage,category,reason,observed});
  assert.equal(p.getSnapshot().mix.phase,'error');assert.equal(p.getSnapshot().mix.tracks.length,0);assert.equal(p.getSnapshot().mix.partial,false);
 }
 assert.equal(messages.length,5);assert.doesNotMatch(JSON.stringify(messages),/private|secret|token|https:|foreign|Homonym|server|\/library\/parts/);
});
test('production mix errors emit no event even with opt-in; broken DEV logging preserves failure and retry',async()=>{
 const s=scope(),messages=[];
 const make=diagnosticSession({DEV:false,VITE_SONANTE_EXPLORE_DIAGNOSTICS:'1'},(...args)=>messages.push(args));
 for(const query of [async()=>{throw Error('private');},async()=>reply(s,'1',null)]){
  const p=make(s,query);await flush();p.openMix();await flush();assert.equal(p.getSnapshot().mix.phase,'error');
 }
 assert.deepEqual(messages,[]);
 let attempts=0,logs=0;
 const broken=diagnosticSession({DEV:true},()=>{logs++;throw Error('console failed with private data');});
 const p=broken(s,async()=>{if(++attempts===1)throw Error('private rejection');return reply(s,'1',[track(1)]);});
 await flush();p.openMix();await flush();assert.equal(logs,1);assert.equal(p.getSnapshot().mix.phase,'error');assert.equal(p.getSnapshot().mix.tracks.length,0);
 p.generateMix();await flush();assert.equal(attempts,2);assert.equal(p.getSnapshot().mix.phase,'ready');assert.equal(p.getSnapshot().mix.tracks.length,1);assert.equal(logs,1);
});
test('all libraries are considered with bounded admission and global capacity, including more than100 sources',async()=>{
 for(const count of [35,105]){
  const s=scope(Array.from({length:count},(_,i)=>String(i+1))),calls=[];let active=0,max=0;const p=create(s,async(server,id,language,limit)=>{active++;max=Math.max(max,active);calls.push({id,limit});await Promise.resolve();active--;return reply(s,id,Array.from({length:limit},(_,i)=>track(i,id)));});await flush();p.openMix();assert.ok(p.queue.length<=4);for(let i=0;i<5;i++)await flush();assert.equal(new Set(calls.map(c=>c.id)).size,count);assert.equal(calls.length,count);assert.equal(max<=2,true);assert.equal(p.getSnapshot().mix.tracks.length,100);
 }
});
test('only explicit Play submits once, reports failure, and stale submission completion cannot affect a new preview',async()=>{
 const s=scope(),p=create(s,async(server,id)=>reply(s,id,[track(1)]));let plays=0;const d=deferred(),play=tracks=>{plays++;assert.equal(tracks.length,1);return d.promise;};await flush();p.openMix();await flush();p.back();p.ensure();assert.equal(plays,0);
 const submission=p.playMix(play);void p.playMix(play);p.generateMix();assert.equal(plays,1);assert.equal(p.getSnapshot().mix.playing,true);d.reject(new Error('private failure'));await submission;assert.equal(p.getSnapshot().mix.playError,true);assert.equal(p.getSnapshot().mix.tracks.length,1);
 const old=deferred();const pending=p.playMix(()=>{plays++;return old.promise;});p.setContext({...s,identity:{}});p.openMix();assert.equal(p.getSnapshot().mix.phase,'idle','pending physical submission still prevents a new generation');old.reject(new Error('stale'));await pending;assert.equal(p.getSnapshot().mix.playError,false);p.openMix();await flush();assert.equal(p.getSnapshot().mix.phase,'ready');assert.equal(plays,2);
});

test('playlist conversion captures exact preview order, source locators and metadata without authenticated URLs',()=>{
 const tracks=[track(3,'2'),{...track(2),duration_ms:undefined,duration:23},track(1)],items=mixPlaylistItems(tracks);
 assert.deepEqual(items.map(i=>i.media_locator.rating_key),['3','2','1']);
 assert.deepEqual(items[0],{media_locator:{...tracks[0].media_locator},metadata:{title:'Homonym',artist:'Artist',album:'Album',duration:120}});
 assert.equal(items[1].metadata.duration,23);assert.notEqual(items[0].media_locator,tracks[0].media_locator);assert.ok(items.every(i=>!('uri' in i)));
});
test('mix Back/reopen preserves selection, both positions and action focus without another query',async()=>{
 const s=scope();let calls=0;const p=create(s,async(_,id)=>{calls++;return reply(s,id,[track(1),track(2)]);});await flush();p.position=400;p.openMix();await flush();const selection=p.getSnapshot().mix.tracks;
 p.mixPosition=720;p.mixFocus='save';p.closeMix();assert.equal(p.getSnapshot().mix.opened,false);p.openMix();await flush();assert.equal(calls,1);assert.equal(p.getSnapshot().mix.tracks,selection);assert.equal(p.position,400);assert.equal(p.mixPosition,720);assert.equal(p.mixFocus,'save');
 p.setContext({...s,identity:{}});assert.equal(p.mixPosition,0);assert.equal(p.mixFocus,null);
});
test('save captures selection, rejects duplicate submission and reports success only after persistence; regeneration leaves saved items intact',async()=>{
 const s=scope();let queries=0,plays=0;const p=create(s,async(_,id)=>reply(s,id,[track(++queries),track(++queries)]));await flush();p.openMix();await flush();const displayed=p.getSnapshot().mix.tracks;
 p.openMixSave();p.setMixSaveName('  Synthetic playlist  ');const write=deferred(),calls=[];const createPlaylist=(name,items)=>{calls.push({name,items});return write.promise;};
 const pending=p.saveMix(createPlaylist);void p.saveMix(createPlaylist);assert.equal(calls.length,1);assert.equal(calls[0].name,'Synthetic playlist');assert.deepEqual(calls[0].items,mixPlaylistItems(displayed));assert.equal(p.getSnapshot().mixSave.busy,true);assert.equal(p.getSnapshot().mixSave.saved,false);
 const saved=structuredClone(calls[0].items);p.closeMix();p.openMix();p.generateMix();await flush();assert.deepEqual(calls[0].items,saved);assert.notEqual(p.getSnapshot().mix.tracks,displayed);assert.equal(plays,0);
 write.resolve({id:'synthetic'});await pending;assert.equal(p.getSnapshot().mixSave.saved,true);assert.equal(p.getSnapshot().mixSave.opened,false);assert.equal(p.getSnapshot().mixSave.busy,false);p.generateMix();await flush();assert.deepEqual(calls[0].items,saved);assert.equal(plays,0);
 await p.playMix(async()=>plays++);assert.equal(plays,1);
});
test('empty name, duplicate name, write failure, retry and cancellation remain visible and never invoke playback',async()=>{
 const s=scope(),p=create(s,async(_,id)=>reply(s,id,[track(1)]));await flush();p.openMix();await flush();let writes=0;
 p.openMixSave();p.setMixSaveName('   ');await p.saveMix(()=>{writes++;assert.fail();});assert.equal(writes,0);assert.equal(p.getSnapshot().mixSave.error,'playlists.nameRequired');
 p.setMixSaveName('A');await p.saveMix(async()=>{writes++;throw 'playlists.duplicateName';});assert.equal(p.getSnapshot().mixSave.error,'playlists.duplicateName');assert.equal(p.getSnapshot().mixSave.saved,false);
 p.setMixSaveName('B');await p.saveMix(async()=>{writes++;throw Error('private details');});assert.equal(p.getSnapshot().mixSave.error,'playlists.createAndAddError');assert.equal(p.getSnapshot().mixSave.busy,false);
 p.cancelMixSave();await p.saveMix(()=>assert.fail('cancelled form'));assert.equal(writes,2);p.openMixSave();p.setMixSaveName('C');await p.saveMix(async()=>writes++);assert.equal(p.getSnapshot().mixSave.saved,true);assert.equal(writes,3);assert.equal(p.getSnapshot().mix.playing,false);
});
test('context switch invalidates old save success/errors and retains physical write lock until settlement',async()=>{
 for(const fail of [false,true]){
  const s=scope(),p=create(s,async(server,id)=>reply({...s,serverId:server},id,[track(1,id,server)]));await flush();p.openMix();await flush();p.openMixSave();p.setMixSaveName('Old');const write=deferred();const pending=p.saveMix(()=>write.promise);
  p.setContext({...s,identity:{}});p.openMix();await flush();p.openMixSave();assert.equal(p.getSnapshot().mixSave.opened,false);assert.equal(p.getSnapshot().mixSave.busy,true);await p.saveMix(()=>assert.fail('physical duplicate write'));
  if(fail)write.reject('playlists.duplicateName');else write.resolve({});await pending;assert.equal(p.getSnapshot().mixSave.saved,false);assert.equal(p.getSnapshot().mixSave.error,null);assert.equal(p.getSnapshot().mixSave.busy,false);p.openMixSave();assert.equal(p.getSnapshot().mixSave.opened,true);
 }
});
