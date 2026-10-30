import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import ts from 'typescript';

const root=fileURLToPath(new URL('../src/',import.meta.url));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const local={uri:'Source/Album/track.flac',title:'Fixture',artist:'Fixture',album:'Fixture'};
const config={first_run:false,audio_output_type:'alsa',local_folders:['/fixture/source']};

// Actual services, intent and scheduler. Native I/O is replaced with a retained
// exclusive read: a transport/Save effect cannot overlap that read. No MPD,
// audio, personal profile or physical music is accessed.
function fixture(){
 const cache=new Map(),calls=[],reads=[],effects=[];
 let busy=false,nativeSave=false,generation=0,controlError;
 const invoke=async(command,args)=>{
  calls.push(command);
  if(command==='admit_playback_intent'){if(nativeSave)throw 'CONFIG_SAVE_BUSY';return String(++generation);}
  if(command==='cancel_playback_intent'){if(args.token===String(generation))generation++;return;}
  if(command==='playback_intent_current')return args.token===String(generation);
  if(['get_local_cover','get_local_covers','get_local_albums'].includes(command)){
   assert.equal(busy,false,'native source reads retain one exclusive reservation');busy=true;
   const pending=deferred();reads.push({command,pending});
   try{const value=await pending.promise;return command==='get_local_covers'?args.paths.map(()=>({Ok:value})):value;}finally{busy=false;}
  }
  if(command==='get_config')return config;
  if(command==='get_playback_status')return {state:'pause'};
  if(['play_tracks','play_uris','toggle_playback','next_track','previous_track','play_queue_index','clear_queue','play_playlist','save_config'].includes(command)){
   if(busy||nativeSave)throw 'CONFIG_SAVE_BUSY';if(controlError)throw controlError;
   if(command!=='save_config')assert.equal(args.intent,String(generation),'no stale gesture effects');
   effects.push({command,args});return command==='save_config'?{status:'accepted',operation:{id:'1',phase:'preparing'}}:undefined;
  }
  assert.fail(`unexpected IPC ${command}`);
 };
 const load=file=>{
  if(cache.has(file))return cache.get(file);
  const exports={};cache.set(file,exports);
  const code=ts.transpileModule(readFileSync(file,'utf8'),{fileName:file,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
  new Function('require','exports',code)(id=>{
   if(id==='@tauri-apps/api/core')return {invoke};
   if(id==='i18next')return {t:key=>key};
   if(id.endsWith('/directVolumeChoice.ts'))return {hasDirectVolumeChoice:()=>true,confirmDirectVolumeConfig:async value=>value};
   if(id.endsWith('/frontendDiagnostics'))return {diagnosticInvoke:(native,command,args)=>native(command,args)};
   assert.ok(id.startsWith('.'),id);return load(resolve(dirname(file),id.endsWith('.ts')?id:`${id}.ts`));
  },exports);return exports;
 };
 const audio=load(resolve(root,'services/audio.ts')).audioService;
 const settings=load(resolve(root,'services/config.ts')).configService;
 const playlists=load(resolve(root,'services/playlists.ts')).playlistsService;
 const intents=load(resolve(root,'utils/playbackIntent.ts'));
 return {audio,settings,playlists,intents,calls,reads,effects,
  set saveBusy(value){nativeSave=value},set error(value){controlError=value},
  release(value='data:image/jpeg;base64,fixture'){assert.ok(reads.length);reads.shift().pending.resolve(value)},
 };
}

for(const command of ['album','toggle','next','previous','queue','uris','playlist','clear'])test(`a current ${command} gesture precedes queued gallery covers without colliding with the active read`,async()=>{
 const f=fixture(),first=f.audio.getPlayerLocalCover('Source/First');await flush();
 const tail=f.audio.getPlayerLocalCover('Source/Queued');
 const run={album:()=>f.audio.playTracks([local],0),toggle:()=>f.audio.togglePlay(),next:()=>f.audio.next(),previous:()=>f.audio.previous(),queue:()=>f.audio.playQueueIndex(0),uris:()=>f.audio.playUris([local.uri],0),playlist:()=>f.playlists.play('fixture'),clear:()=>f.audio.clearQueue()}[command];
 const result=run();void result.catch(()=>{});await flush();assert.equal(f.effects.length,0);
 f.release();await first;await flush();
 try{assert.equal(f.effects.length,1,'foreground action must not wait for the gallery');assert.equal(f.reads.length,1);}finally{if(f.reads.length)f.release();await tail;}
 await result;
});

test('folder-removal Save uses the same queue and retains its exact authorization',async()=>{
 const f=fixture(),first=f.audio.getPlayerLocalCover('Source/First');await flush();
 const tail=f.audio.getPlayerLocalCover('Source/Queued'),authorization={operation_id:'fixture-operation',binding:'a'.repeat(64)},next={...config,local_folders:[]};
 const saving=f.settings.saveConfig(next,authorization);void saving.catch(()=>{});await flush();assert.equal(f.effects.length,0);
 f.release();await first;await flush();
 try{assert.deepEqual(f.effects,[{command:'save_config',args:{newConfig:next,queueClearAuthorization:authorization}}]);}finally{if(f.reads.length)f.release();await tail;}
 assert.equal((await saving).status,'accepted');
});

test('superseded or cancelled playback waiting for a read performs no effect',async()=>{
 for(const cancel of [false,true]){
  const f=fixture(),cover=f.audio.getPlayerLocalCover('Source/First');await flush();
  const intent=f.intents.beginPlaybackIntent(),old=f.audio.playTracks([local],0,intent);void old.catch(()=>{});await flush();
  if(cancel)intent.cancel();else{const newer=f.intents.beginPlaybackIntent();await newer.token;}
  f.release();await cover;await assert.rejects(old,error=>error==='PLAYBACK_SUPERSEDED');assert.deepEqual(f.effects,[]);
 }
});

test('a genuine native refusal is preserved and never replayed when the operation becomes idle',async()=>{
 const f=fixture();f.saveBusy=true;await assert.rejects(f.audio.playTracks([local],0),error=>error==='CONFIG_SAVE_BUSY');
 f.saveBusy=false;await flush();assert.deepEqual(f.effects,[]);
 f.error='MANAGED_FOLDERS_SAVE_PENDING';await assert.rejects(f.settings.saveConfig(config),error=>error==='MANAGED_FOLDERS_SAVE_PENDING');
 f.error=undefined;await flush();assert.deepEqual(f.effects,[]);
});

test('a failed read releases the foreground gesture without retrying that read',async()=>{
 const f=fixture(),cover=f.audio.getPlayerLocalCover('Source/First');void cover.catch(()=>{});await flush();
 const play=f.audio.playTracks([local],0);void play.catch(()=>{});await flush();f.reads.shift().pending.reject('SOURCE_AVAILABILITY_CHANGED');
 await assert.rejects(cover,error=>error==='SOURCE_AVAILABILITY_CHANGED');await play;
 assert.equal(f.effects.length,1);assert.equal(f.calls.filter(c=>c==='get_local_cover').length,1);
});
