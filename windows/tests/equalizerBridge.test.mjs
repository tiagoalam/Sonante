import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs/promises';import ts from 'typescript';
const source=await fs.readFile(new URL('../src/compat.ts',import.meta.url),'utf8');
const {createWindowsBridge,playbackProjection}=await import('data:text/javascript;base64,'+Buffer.from(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText).toString('base64'));
test('original EQ catalog/editor/import/export commands reach only their native counterparts without starting playback',async()=>{
 const calls=[];const sentinel={own:'native result'};
 const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});return sentinel},()=>({getItem:()=>assert.fail('no WebView profile'),setItem:()=>assert.fail('no WebView profile')}));
 for(const [name,args] of [['get_equalizer',{}],['edit_equalizer',{expectedRevision:'fixture-revision',edit:{action:'select',reference:{kind:'built_in',id:'flat'}}}],['pick_equalizer_preset',{}],['export_equalizer_preset',{preset:{name:'Fixture'}}]]){
  assert.equal(await bridge(name,args),sentinel);assert.deepEqual(calls.at(-1),{name,args});
 }
 assert.equal(calls.length,4);assert.equal(calls.some(c=>c.name==='windows_action'||c.name==='windows_intent'),false);
});
test('apply and comparison retain the original typed preset/level-return/token contract without renderer runtime or MPD commands',async()=>{
 const calls=[];const snapshot={phase:'ready',verified:false};
 const bridge=createWindowsBridge(async(name,args)=>{calls.push({name,args});return snapshot},()=>({getItem:()=>assert.fail(),setItem:()=>assert.fail()}));
 const preset={id:'own-tonal-fixture',name:'Own fixture',preamp_db:-4,bands:[]};
 for(const [name,args]of[['apply_equalizer',{enabled:true,preset,bypassExpected:null}],['apply_equalizer',{enabled:false,preset:null,bypassExpected:preset}],['compare_equalizer',{action:'reference',token:null,expected:preset}],['compare_equalizer',{action:'finish',token:'cmp-own-fixture',expected:null}]]){
  assert.equal(await bridge(name,args),snapshot);assert.deepEqual(calls.at(-1),{name,args});
 }
 assert.equal(calls.length,4);
});
test('DSP status is projected exactly while physical output and engine qualification remain unverified',()=>{
 const equalizer={revision:4,phase:'ready',applied:null,comparison:null,error:null,pcm_rate:48000,pcm_channels:2,verified:false,in_path:true};
 const native={generation:3,engineRunning:true,state:'pause',elapsed:20,duration:200,volume:30,audio:'',warning:null,queue:[{id:'own',disc:1,title:'Own fixture',artist:'Synthetic',album:'Fixture',fileName:'own.wav'}],queueIndex:0,equalizer};
 const p=playbackProjection(native,false);assert.equal(p.shared_dsp,equalizer);assert.equal(p.shared_dsp.verified,false);assert.equal(p.state,'pause');assert.equal(p.audio_format,'');assert.equal(p.output_observation.state,'unverified');assert.equal(p.engine_evidence,null);assert.equal(p.processing,null);
 assert.equal(playbackProjection({...native,equalizer:undefined},false).shared_dsp,null);
});
