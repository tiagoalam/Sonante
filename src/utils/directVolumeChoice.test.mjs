import test,{afterEach}from'node:test';import assert from'node:assert/strict';import{readFileSync}from'node:fs';import ts from'typescript';import i18n from'i18next';
import{directVolumeContext,hasDirectVolumeChoice,confirmDirectVolumeConfig}from'./directVolumeChoice.ts';import{ensureDirectOutputChoice}from'../services/config.ts';import * as intents from'./playbackIntent.ts';
const base=()=>({first_run:false,audio_output_type:'alsa',alsa_device:'hw:CARD=Fixture,DEV=0',dop_enabled:false,local_folders:[],direct_volume_choice:null,direct_muted:false});
const prior=globalThis.window;afterEach(()=>{globalThis.window=prior;});
await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(l=>[l,{translation:JSON.parse(readFileSync(new URL(`../locales/${l}.json`,import.meta.url)))}]))});
let fixtureSerial=0;
function fixture({accept=true,playing=false}={}){
 let cfg=base(),generation=++fixtureSerial*1000,prompts=0;const calls=[];const native=async(command,args)=>{
  calls.push([command,args]);if(command==='admit_playback_intent')return String(++generation);if(command==='playback_intent_current')return args.token===String(generation);if(command==='cancel_playback_intent'){if(args.token===String(generation))generation++;return;}
  if(command==='get_config')return structuredClone(cfg);if(command==='get_playback_status')return{state:playing?'play':'stop'};
  if(command==='confirm_direct_volume_choice'){assert.deepEqual(args.expectedConfig,cfg);assert.equal(args.intent,String(generation));cfg={...cfg,direct_volume_choice:directVolumeContext(cfg)};return;}
  if(command==='save_config'){cfg=args.newConfig;return{status:'completed'}}
 };
 globalThis.window={confirm:message=>{prompts++;assert.equal(message,i18n.t('player.directVolumeChoiceConfirm'));return accept;},__TAURI_INTERNALS__:{invoke:native}};
 const js=ts.transpileModule(readFileSync(new URL('../services/audio.ts',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const exports={};
 const deps={'./config':{ensureDirectOutputChoice},'../utils/playbackIntent':intents,'@tauri-apps/api/core':{invoke:native},'../utils/frontendDiagnostics':{diagnosticInvoke:(fn,c,a)=>fn(c,a)}};
 new Function('require','exports',js)(id=>{assert.ok(id in deps,id);return deps[id]},exports);
 return{service:exports.audioService,calls,get cfg(){return cfg},set cfg(v){cfg=v},get prompts(){return prompts},native};
}
test('choice is versioned, bound to endpoint/DoP, and cancelling preserves draft byte-for-byte',async()=>{
 const c=base(),before=JSON.stringify(c);await assert.rejects(confirmDirectVolumeConfig(c,()=>false),e=>e==='DIRECT_CHOICE_CANCELLED');assert.equal(JSON.stringify(c),before);
 const granted=await confirmDirectVolumeConfig(c,()=>true);assert.ok(hasDirectVolumeChoice(granted));for(const patch of[{alsa_device:'hw:Other,0'},{dop_enabled:true},{direct_volume_choice:{...granted.direct_volume_choice,version:2}}])assert.equal(hasDirectVolumeChoice({...granted,...patch}),false);
 const shared={...granted,audio_output_type:'pipewire'};assert.equal((await confirmDirectVolumeConfig(shared,()=>assert.fail())).direct_volume_choice,null);
});
test('first ordinary Play migrates once; later Play, album/track/queue and next/previous use the current intent without new confirmation',async()=>{
 for(const language of['en','pt-BR']){await i18n.changeLanguage(language);const f=fixture();
  await f.service.togglePlay();assert.equal(f.prompts,1);assert.ok(hasDirectVolumeChoice(f.cfg));
  await f.service.togglePlay();await f.service.playUris(['fixture.wav'],0);await f.service.playTracks([{uri:'fixture.wav'}],0);await f.service.playQueueIndex(0);await f.service.next();await f.service.previous();
  assert.equal(f.prompts,1);const actual=f.calls.filter(([c])=>['toggle_playback','play_uris','play_tracks','play_queue_index','next_track','previous_track'].includes(c));assert.equal(actual.length,7);for(const[,a]of actual)assert.ok(a.intent);assert.equal(actual[0][1].wantPlay,true);assert.ok(!JSON.stringify(f.calls).includes('prepare'));
 }
});
test('cancelled migration does not persist or send playback; Pause never requires a volume-choice dialog',async()=>{
 let f=fixture({accept:false});await assert.rejects(f.service.togglePlay(),e=>e==='PLAYBACK_SUPERSEDED');assert.equal(f.prompts,1);assert.ok(!f.calls.some(([c])=>['toggle_playback','save_config','confirm_direct_volume_choice'].includes(c)));
 f=fixture({playing:true});await f.service.togglePlay();assert.equal(f.prompts,0);assert.ok(!f.calls.some(([c])=>c==='get_config'));assert.equal(f.calls.find(([c])=>c==='toggle_playback')[1].wantPlay,false);
});
test('a real context change prompts again; buffer/ReplayGain changes retain this volume policy only',async()=>{
 const f=fixture();await f.service.togglePlay();f.cfg={...f.cfg,audio_buffer_size_kb:8192,replay_gain:'track'};await f.service.togglePlay();assert.equal(f.prompts,1);f.cfg={...f.cfg,dop_enabled:true};await f.service.togglePlay();assert.equal(f.prompts,2);
});
test('a newer intent during choice persistence prevents the old requested Play',async()=>{
 const f=fixture();let release;const pending=new Promise(r=>release=r),native=f.native;
 window.__TAURI_INTERNALS__.invoke=async(c,a)=>{if(c==='confirm_direct_volume_choice'){await pending;return native(c,a);}return native(c,a)};
 const intent=intents.beginPlaybackIntent();const choosing=ensureDirectOutputChoice(intent);for(let i=0;i<20;i++)await Promise.resolve();const newer=intents.beginPlaybackIntent();await newer.token;release();await assert.rejects(choosing);assert.ok(!f.calls.some(([c])=>c==='toggle_playback'));
});
