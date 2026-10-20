import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {canToggleMute,directMuteRequest,muteLabel,requiresDeviceVolumeControl} from './signalPath.ts';
import {routeConditions} from './routeConditions.ts';
const volume={backend:'direct_equipment',available:true,writable:false,value:0,muted:false,direct_gain:'pending',direct_token:'session:revision'};
const status=(extra={})=>({state:'play',audio_format:'44100:16:2',volume:{...volume,...extra}});
test('all Direct formats have equipment volume; Shared software and system volume remain adjustable',()=>{
 for(const audio_format of ['', '44100:16:2','192000:24:2','dsd64:2','352800:dsd:2']){
  assert.equal(requiresDeviceVolumeControl({mode:'direct'},{...status(),audio_format}),true);
  for(const backend of ['mpd_software','pipewire']) assert.equal(requiresDeviceVolumeControl({mode:'shared'},{...status(),audio_format,volume:{...volume,backend}}),false);
 }
});
test('all Direct mute actions are explicit; no separate preparation gesture',()=>{
 for(const direct_gain of ['pending','changed','unity','muted'])for(const muted of [false,true]){
  const s=status({direct_gain,muted});assert.equal(muteLabel(s),muted?'player.unmute':'player.mute');
  assert.deepEqual(directMuteRequest(s),{muted:!muted,token:volume.direct_token});
 }
 for(const extra of [{available:false},{direct_token:undefined},{backend:'unavailable'},{backend:'mpd_software'}])assert.equal(directMuteRequest(status(extra)),null);
 assert.equal(canToggleMute(status({backend:'pipewire',writable:true})),true);
});
test('Direct evidence requires real unity or coherent intentional mute for PCM, native DSD and DoP',()=>{
 const route={mode:'direct',endpoint:'hw:Fixture,0'},health={state:'available'};
 const engine_evidence={state:'identified',evidence_id:'mpd-strict-ubuntu2204-remap-20261018',scope:'pcm_dsd_route_strict',pcm_eof_flush:true};
 for(const [audio_format,kind,format,rate] of [['44100:16:2','pcm','S16_LE',44100],['352800:dsd:2','native_dsd','DSD_U32_LE',88200],['352800:dsd:2','dop','S32_LE',176400]]){
  const base={...status(),audio_format,snapshot_consistent:true,engine_evidence,output_observation:{state:'observed',configured_endpoint:route.endpoint,card:0,device:0,subdevice:0,format,rate,channels:2},processing:{transport:{kind,source_rate:352800,output_rate:rate,channels:2,epoch:1},mpd_volume:100,crossfade_seconds:0,mixramp:'off',replay_gain:{state:'off'}}};
  const evaluate=(gain,value,reported=value,muted=false)=>routeConditions(route,{...base,volume:{...volume,direct_gain:gain,value,muted},processing:{...base.processing,mpd_volume:reported}},health);
  assert.equal(evaluate('unity',100).bitPerfect,true,JSON.stringify({audio_format,kind,result:evaluate('unity',100)}));
  assert.equal(evaluate('muted',0,0,true).bitPerfect,true);
  for(const [gain,value,reported,muted] of [['pending',100,100,false],['changed',100,100,false],['unity',43,43,false],['unity',100,0,false],['muted',0,0,false],['muted',0,100,true]])assert.equal(evaluate(gain,value,reported,muted).bitPerfect,false);
 }
});
test('central gradual IPC and MPRIS cannot bypass Direct policy; all frontends use dedicated mute',()=>{
 const root=new URL('../../',import.meta.url),read=n=>readFileSync(new URL(n,root),'utf8');
 const mpris=read('src-tauri/src/mpris.rs');assert.match(mpris,/async fn set_volume\([\s\S]*?Err\(zbus::Error::Unsupported\)/);
 for(const file of ['src/App.tsx','src/components/PlayerBar.tsx','src/components/NowPlayingHost.tsx']){const code=read(file);assert.match(code,/directMuteRequest/);assert.match(code,/audioService.setDirectMute/);}
 const lib=read('src-tauri/src/lib.rs');assert.match(lib,/require_gradual_volume\(supervisor.volume_backend\(\)\)\?/);
 const sup=read('src-tauri/src/supervisor.rs');assert.doesNotMatch(sup,/detect_for_pcm\(/);
});

test('unconfirmed external attenuation can always be explicitly muted without raising gain',()=>{const s=status({direct_gain:'changed',value:43});assert.equal(muteLabel(s),'player.mute');assert.deepEqual(directMuteRequest(s),{muted:true,token:volume.direct_token});});

test('actual keyboard volume branches block Direct arrows; M requests only explicit mute',async()=>{
 const ts=(await import('typescript')).default;const code=readFileSync(new URL('../App.tsx',import.meta.url),'utf8');const ast=ts.createSourceFile('App.tsx',code,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let body;
 const visit=n=>{if(ts.isVariableDeclaration(n)&&n.name.getText(ast)==='handleKeyDown'){const text=n.initializer.body.getText(ast);const at=text.indexOf('const current = statusRef.current;');if(at>=0)body=text.slice(at,-1);}ts.forEachChild(n,visit)};visit(ast);assert.ok(body);
 const handler=new Function('e','statusRef','audioService','logError','canToggleMute','directMuteRequest','t','window','playerNavigationRequests',ts.transpileModule(body,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText);
 for(const code of ['ArrowUp','ArrowDown','KeyM']){
  const calls=[];handler({code,key:code==='KeyM'?'m':'ArrowUp',preventDefault(){}},{current:status()},{setVolume:v=>{calls.push(['volume',v]);return Promise.resolve()},setDirectMute:r=>{calls.push(['mute',r]);return Promise.resolve()}},()=>assert.fail(),canToggleMute,directMuteRequest,k=>k,{confirm:()=>assert.fail('no separate prepare dialog'),alert:()=>assert.fail()},{});
  await Promise.resolve();assert.deepEqual(calls,code==='KeyM'?[['mute',{muted:true,token:volume.direct_token}]]:[]);
 }
});
