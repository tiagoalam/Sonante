import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs/promises';import ts from 'typescript';
const source=await fs.readFile(new URL('../src/compat.ts',import.meta.url),'utf8');
const {createWindowsBridge,playbackProjection}=await import('data:text/javascript;base64,'+Buffer.from(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText).toString('base64'));
const endpoint='hw:windows:'+'a'.repeat(64),audio={audio_output_type:'alsa',alsa_device:endpoint,dop_enabled:false,audio_buffer_size_kb:4096,replay_gain:'off',direct_volume_choice:{version:1,endpoint,dop_enabled:false},direct_muted:false};
function fixture(){const calls=[];const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});if(name==='windows_preferences')return{roots:['C:/OwnFixture'],sources:{local:true,navidrome:false,plex:false},plex:{url:'',server_id:null,server_name:null,saved:false,revision:null},audio};if(name==='windows_configure_sources')return{status:'accepted',operation:{id:'23',phase:'indexing',error:null,confirmed_playback:false}};return null},()=>({getItem:()=>assert.fail('no browser profile'),setItem:()=>assert.fail('no browser profile')}));return{bridge,calls};}
test('reading route settings neither launches an engine nor admits playback and retains the exact Direct context',async()=>{const{bridge,calls}=fixture();const c=await bridge('get_config');assert.equal(c.alsa_device,endpoint);assert.deepEqual(c.direct_volume_choice,audio.direct_volume_choice);assert.deepEqual(calls.map(c=>c.name),['windows_preferences']);assert.equal(c.plex_token,'');});
test('audio and source changes participate in one native configuration operation without renderer credentials or runtime paths',async()=>{const{bridge,calls}=fixture();const c=await bridge('get_config');const response=await bridge('save_config',{newConfig:{...c,audio_output_type:'pipewire',local_folders:['C:/OwnFixture','D:/OwnFixture'],sources:{local:true,navidrome:true,plex:false}}});assert.equal(response.status,'accepted');assert.equal(response.operation.id,'23');const writes=calls.filter(c=>c.name==='windows_configure_sources');assert.equal(writes.length,1);assert.equal(writes[0].args.audio.audio_output_type,'pipewire');assert.deepEqual(writes[0].args.roots,['C:/OwnFixture','D:/OwnFixture']);assert.equal(writes[0].args.sources.navidrome,true);assert.equal(writes[0].args.plexAccount,null);assert.equal('plex_token' in writes[0].args.audio,false);assert.equal('runtime' in writes[0].args.audio,false);});
test('Direct volume uses native owned tokens and never publishes a software slider or fabricated hardware observation',()=>{const p={generation:7,engineRunning:true,state:'play',elapsed:2,duration:20,volume:100,audio:'44100:16:2',warning:null,queue:[{id:'fixture',disc:1,title:'Own',artist:'Fixture',album:'Synthetic',fileName:'own.wav'}],queueIndex:0,audioSettings:audio,direct:{token:'1:3',gain:'unity',muted:false,available:true,consistent:true}};const s=playbackProjection(p,false);assert.equal(s.volume.backend,'direct_equipment');assert.equal(s.volume.writable,false);assert.equal(s.volume.direct_token,'1:3');assert.equal(s.volume.shared_token,undefined);assert.equal(s.output_observation.state,'unverified');assert.equal(s.engine_evidence,null);const quiet=playbackProjection({...p,engineRunning:false,state:'stop',direct:null,logicalState:'stop'},false);assert.equal(quiet.state,'stop');assert.equal(quiet.volume.available,false);assert.equal(quiet.volume.direct_gain,'pending');});
test('Windows route projection reaches the existing playback snapshot without claiming WASAPI exclusivity',()=>{
 const audioRoute={generation:9,operationId:'windows-session:3',operation:'stable',route:{state:'requested',requested:{destination:{kind:'device',deviceId:endpoint},access:'exclusive',processing:'off'},confirmed:null},applied:{destination:{kind:'device',deviceId:endpoint},access:'exclusive',processing:'off'},evidence:['persisted_configuration','supervisor_output_applied','windows_session_observed'],diagnostic:'windows_exclusive_not_observable',pcmAcquired:null,exclusiveAccessConfirmed:null,legacyIssues:[]};
 const p={generation:3,engineRunning:true,state:'play',elapsed:2,duration:20,volume:100,audio:'44100:16:2',warning:null,queue:[{id:'fixture',disc:1,title:'Own',artist:'Fixture',album:'Synthetic',fileName:'own.wav'}],queueIndex:0,audioSettings:audio,direct:{token:'1:3',gain:'unity',muted:false,available:true,consistent:true},audioRoute};
 assert.deepEqual(playbackProjection(p,false).audio_route,audioRoute);
});
test('confirmation and mute use the dedicated native safety commands and pass no general MPD text',async()=>{const{bridge,calls}=fixture();const c=await bridge('get_config');await bridge('confirm_direct_volume_choice',{expectedConfig:c,intent:'17'});assert.deepEqual(calls.at(-1),{name:'windows_confirm_direct',args:{expected:audio,intent:'17'}});await bridge('set_direct_mute',{muted:true,token:'1:3'});assert.deepEqual(calls.at(-1),{name:'windows_direct_mute',args:{muted:true,token:'1:3'}});});
test('Shared gain reaches native admission immediately without waiting for a status query or browser profile',async()=>{
 const calls=[];const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});assert.equal(name,'windows_gain','a queued status must not delay gain admission');return{generation:7,volume:23}},()=>assert.fail('no browser profile'));
 await bridge('set_volume',{volume:23});assert.deepEqual(calls,[{name:'windows_gain',args:{action:{kind:'volume',value:23}}}]);
});
test('busy gain is refused once without deferred status, playback intent or fallback action',async()=>{
 const calls=[];const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});throw 'engine_busy'},()=>assert.fail('no browser profile'));
 await assert.rejects(bridge('set_volume',{volume:41}),error=>error==='engine_busy');assert.deepEqual(calls.map(c=>c.name),['windows_gain']);
});
test('Shared mute carries the displayed owner token unchanged to native admission',async()=>{
 const calls=[];const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});assert.equal(name,'windows_gain');throw 'SHARED_MUTE_CONTEXT_CHANGED'},()=>assert.fail('no browser profile'));
 await assert.rejects(bridge('set_shared_mute',{muted:true,token:'windows:7'}),error=>error==='SHARED_MUTE_CONTEXT_CHANGED');
 assert.deepEqual(calls,[{name:'windows_gain',args:{action:{kind:'mute',muted:true,token:'windows:7'}}}]);
 await assert.rejects(bridge('set_shared_mute',{muted:true,token:null}),error=>error==='invalid_argument');assert.equal(calls.length,1);
});
test('an older gain response cannot overwrite a newer completed gesture',async()=>{
 let resolveOld;const pending=new Promise(resolve=>{resolveOld=resolve});let calls=0;
 const bridge=createWindowsBridge(async(name,args)=>{assert.equal(name,'windows_gain');calls++;return args.action.value===20?pending:{generation:7,volume:90}},()=>assert.fail('no browser profile'));
 const old=bridge('set_volume',{volume:20});const stale=assert.rejects(old,error=>error==='PLAYBACK_SUPERSEDED');
 await bridge('set_volume',{volume:90});resolveOld({generation:7,volume:20});await stale;assert.equal(calls,2);
});
test('a missing restored random order is a fixed public code and never an authenticated/raw message',async()=>{
 const source=await fs.readFile(new URL('../src/errorCode.ts',import.meta.url),'utf8');
 const {publicErrorCode}=await import('data:text/javascript;base64,'+Buffer.from(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText).toString('base64'));
 const code='PLAYBACK_RANDOM_ORDER_UNAVAILABLE';assert.equal(publicErrorCode(code),code);
 for(const value of [code+'?token=PRIVATE',code+' PRIVATE','PRIVATE'+code,{code},new String(code),'https://private/?token=PRIVATE'])assert.equal(publicErrorCode(value),null);
});
test('a refused restored-order gesture is propagated once without fallback playback or gain',async()=>{
 const calls=[];const code='PLAYBACK_RANDOM_ORDER_UNAVAILABLE';
 const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});if(name==='windows_status')return{generation:7};assert.equal(name,'windows_action');throw code},()=>assert.fail('no browser profile'));
 await assert.rejects(bridge('next_track',{intent:'own-17'}),error=>error===code);
 assert.deepEqual(calls,[{name:'windows_status',args:undefined},{name:'windows_action',args:{generation:7,action:{kind:'next'},intent:'own-17'}}]);
});
