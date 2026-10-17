import test from 'node:test';
import assert from 'node:assert/strict';
import {routeConditions} from './routeConditions.ts';
import {readFileSync} from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import ts from 'typescript';
import i18next from 'i18next';
import {signalPathInformation,requiresDeviceVolumeControl} from './signalPath.ts';
const route={mode:'direct',endpoint:'hw:CARD=Fixture,DEV=0'},health={state:'available'};
const observation={state:'observed',configured_endpoint:route.endpoint,card:2,device:0,subdevice:0,format:'S32_LE',rate:96000,channels:2};
const processing={mpd_volume:100,crossfade_seconds:0,mixramp:'off',replay_gain:{state:'off'}};
const playback={state:'play',audio_format:'96000:24:2',current_media:{kind:'local',queue_index:0,uri:'fixture.flac'},output_observation:observation,processing,volume:{backend:'mpd_software',available:true,value:100}};
const row=(status,label,selected=route)=>routeConditions(selected,status,health).rows.find(row=>row.label===label);

test('same rate/channels and a wider integer container never produce a bit-perfect verdict',()=>{
 const before=structuredClone(playback);const result=routeConditions(route,playback,health);
 assert.equal(result.summary,'preserved');assert.equal(row(playback,'rate').message,'rateEqual');assert.equal(row(playback,'channels').message,'channelsEqual');
 assert.equal(row(playback,'container').message,'containerWider');assert.equal(row(playback,'container').tone,'unknown');assert.equal(row(playback,'volume').message,'softwareUnity');assert.deepEqual(playback,before);
 for(const bits of [16,24,32]){const status={...playback,audio_format:`96000:${bits}:2`};assert.equal(routeConditions(route,status,health).summary,'preserved');}
 assert.equal(result.bitPerfect,false);assert.doesNotMatch(JSON.stringify(result),/certified|verified|eligible/i);
});
test('actual format differences and configured processing are explained separately',()=>{
 const status={...playback,audio_format:'192000:32:2',output_observation:{...observation,format:'S16_LE',rate:48000,channels:1},processing:{...processing,mpd_volume:43,crossfade_seconds:2,replay_gain:{state:'album'}}};
 assert.equal(routeConditions(route,status,health).summary,'attention');
 for(const [label,message] of [['rate','rateDifferent'],['channels','channelsDifferent'],['container','containerSmaller'],['volume','softwareVolume'],['replayGain','gain_album'],['crossfade','crossfadeOn']]){assert.equal(row(status,label).message,message);assert.equal(row(status,label).tone,'attention');}
 for(const state of ['track','album','auto'])assert.equal(row({...playback,processing:{...processing,replay_gain:{state}}},'replayGain').message,`gain_${state}`);
});
test('missing values, inaccessible ReplayGain, hardware/system volume and unsupported containers remain unknown',()=>{
 const absent={...playback,processing:undefined,volume:{...playback.volume,value:100}};
 assert.equal(row(absent,'volume').message,'notChecked');assert.equal(row(absent,'crossfade').message,'notChecked');assert.equal(row(absent,'replayGain').message,'notChecked');
 for(const reason of ['busy','unavailable','timeout','protocol','changed'])assert.equal(row({...playback,processing:{...processing,replay_gain:{state:'unavailable',reason}}},'replayGain').tone,'unknown');
 for(const [backend,message] of [['alsa_hardware','hardwareVolume'],['pipewire','systemVolume']]){const status={...playback,volume:{...playback.volume,backend,value:100}};assert.equal(row(status,'volume').message,message);assert.equal(row(status,'volume').tone,'unknown');}
 for(const format of ['FLOAT_LE','UNKNOWN'])assert.equal(row({...playback,output_observation:{...observation,format}},'container').message,'containerUnknown');
 for(const invalid of [null,NaN,Infinity,-1,101])assert.equal(row({...playback,processing:{...processing,mpd_volume:invalid}},'volume').message,'notChecked');
 assert.equal(row({...playback,processing:{...processing,mixramp:'possible'}},'mixramp').tone,'unknown');
});
test('DSD and DoP never compare PCM carrier rates/depths or assume software gain behavior',()=>{
 for(const audio_format of ['dsd64:2','352800:dsd:2','48000:dop:2']){
  const status={...playback,audio_format,processing:{...processing,mpd_volume:43}};for(const label of ['rate','channels','container'])assert.equal(row(status,label).tone,'unknown');assert.equal(row(status,'volume').message,'softwareOther');
 }
 const native={...playback,output_observation:{...observation,format:'DSD_U32_LE'}};assert.equal(row(native,'rate').message,'dsdSeparate');
});
test('paused/stopped/unavailable and mismatched output evidence cannot keep current verification rows',()=>{
 for(const state of ['pause','stop'])assert.deepEqual(routeConditions(route,{...playback,state},health),{summary:'inactive',rows:[]});
 assert.deepEqual(routeConditions(route,playback,{state:'unavailable'}),{summary:'inactive',rows:[]});
 assert.equal(row(playback,'rate',{mode:'direct',endpoint:'hw:Other,0'}).message,'notChecked');
 assert.equal(row(playback,'route',{mode:'shared',endpoint:'default'}).message,'routeShared');
 for(const evidence of [undefined,{state:'unverified',reason:'changed'},{...observation,rate:NaN}])assert.equal(row({...playback,output_observation:evidence},'rate').message,'notChecked');
});
function compile(t,view='SignalPathDetails',hooks=React){const source=readFileSync(new URL('../components/SignalPath.tsx',import.meta.url),'utf8');const exports={};const deps={react:{...hooks,default:hooks},'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t})},'../utils/signalPath':{signalPathInformation,requiresDeviceVolumeControl},'../utils/routeConditions':{routeConditions}};new Function('require','exports',ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);return exports[view];}
function nodes(tree){return React.isValidElement(tree)?[tree,...nodes(tree.props.children)]:Array.isArray(tree)?tree.flatMap(nodes):[];}
test('real SignalPath shows translated scoped conditions in both languages and hosts without adding playback handlers',async()=>{
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});const t=i18n.t.bind(i18n),SignalPath=compile(t);
  for(const horizontal of [false,true])for(const kind of ['local','plex']){
   const html=renderToStaticMarkup(React.createElement(SignalPath,{route,status:{...playback,current_media:{...playback.current_media,kind}},health,horizontal}));
   assert.ok(html.includes(t('signalPath.conditions.title')));assert.ok(html.includes(t('signalPath.conditions.value.containerWider',{input:24,output:32})));assert.ok(html.includes(t('signalPath.observation.notVerified'))||html.includes(t('signalPath.seal.scope')));assert.doesNotMatch(html,/signalPath\.|data-bitperfect/);
   const tree=SignalPath({route,status:playback,health,horizontal});const details=nodes(tree).find(n=>n.type==='section'&&'data-route-conditions' in n.props);assert.ok(details);assert.equal(details.props.open,undefined,'closed by default');
   const panel=nodes(details).find(n=>n.props.role==='region');assert.equal(panel.props.tabIndex,0);assert.equal(panel.props['aria-label'],t('signalPath.conditions.title'));

  }
 }
});

test('bit-perfect route badge additionally requires the exact identified engine and current conditions',()=>{
 const engine_evidence={state:'identified',evidence_id:'mpd-pcm-eof-20261017',scope:'pcm24_96000_stereo_gapless',pcm_eof_flush:true};
 const status={...playback,engine_evidence};
 assert.equal(row(status,'engine').message,'engineIdentified');assert.equal(row(status,'engine').tone,'info');assert.equal(routeConditions(route,status,health).bitPerfect,true);
 assert.equal(routeConditions(route,status,health).summary,'preserved');
 assert.equal(routeConditions(route,{...status,processing:{...processing,mpd_volume:50}},health).summary,'attention');
 for(const audio_format of ['44100:16:2','192000:24:2','dsd64:2','unknown']){
  assert.equal(row({...status,audio_format},'engine').message,'engineIdentified');
  assert.equal(row({...status,audio_format},'engine').tone,'info');
 }
 for(const engine of [undefined,{...engine_evidence,evidence_id:'other'},{...engine_evidence,scope:'other'},{...engine_evidence,pcm_eof_flush:false}])assert.equal(row({...status,engine_evidence:engine},'engine').message,'notChecked');
 assert.equal(row(status,'engine',{mode:'shared',endpoint:'default'}).message,'notChecked');
 assert.equal(row(status,'engine',{mode:'direct',endpoint:'hw:Other,0'}).message,'notChecked');
 for(const state of ['pause','stop'])assert.deepEqual(routeConditions(route,{...status,state},health),{summary:'inactive',rows:[]});
});

test('identified engine and checked route scope render in Portuguese and English',async()=>{
 const engine_evidence={state:'identified',evidence_id:'mpd-pcm-eof-20261017',scope:'pcm24_96000_stereo_gapless',pcm_eof_flush:true};
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});
  const t=i18n.t.bind(i18n),SignalPath=compile(t);
  const html=renderToStaticMarkup(React.createElement(SignalPath,{route,status:{...playback,engine_evidence},health}));
  assert.ok(html.includes(t('signalPath.conditions.label.engine')));
  assert.ok(html.includes(t('signalPath.conditions.value.engineIdentified')));
  assert.ok(html.includes(t('signalPath.observation.notVerified'))||html.includes(t('signalPath.seal.scope')));
  assert.doesNotMatch(html,/signalPath\.|data-bitperfect/);
 }
});

const measuredPlayback={...playback,output_observation:{...observation,kernel_release:'6.12.108-1-MANJARO',usb_device:{vendor_id:'152a',product_id:'8750',model:'E70'}},engine_evidence:{state:'identified',evidence_id:'mpd-pcm-eof-20261017',scope:'pcm24_96000_stereo_gapless',pcm_eof_flush:true}};
test('live assessment requires observed PCM and processing, without device or kernel whitelists',()=>{
 const before=structuredClone(measuredPlayback);
 assert.equal(routeConditions(route,measuredPlayback,health).assessment,'compatible');
 const changes=[
  {audio_format:'dsd64:2'}, {audio_format:'unknown'},
  {output_observation:{...observation,format:'FLOAT_LE'}},
  {output_observation:{...observation,configured_endpoint:'hw:Other,0'}},
  {output_observation:{...observation,card:-1}},
  {output_observation:{...observation,subdevice:NaN}},
  {processing:undefined},
  {processing:{...processing,mpd_volume:undefined}},
  {processing:{...processing,replay_gain:{state:'unknown'}}},
  {processing:{...processing,crossfade_seconds:undefined}},
  {processing:{...processing,mixramp:'possible'}},
  {volume:{...playback.volume,value:50}},
  {volume:{...playback.volume,available:false}},
  {volume:{...playback.volume,backend:'alsa_hardware'}},
 ];
 const changed=change=>({...measuredPlayback,...change,...(change.output_observation?{output_observation:{...measuredPlayback.output_observation,...change.output_observation}}:{})});
 for(const change of changes)assert.equal(routeConditions(route,changed(change),health).assessment,'partial',JSON.stringify(change));
 for(const change of [
  {processing:{...processing,mpd_volume:50}},
  {processing:{...processing,replay_gain:{state:'album'}}},
  {processing:{...processing,crossfade_seconds:1}},
  {output_observation:{...observation,rate:48000}},
  {output_observation:{...observation,channels:1}},
 ])assert.equal(routeConditions(route,changed(change),health).assessment,'attention');
 for(const state of ['pause','stop'])assert.equal(routeConditions(route,{...measuredPlayback,state},health).assessment,undefined);
 assert.equal(routeConditions(route,measuredPlayback,{state:'unavailable'}).assessment,undefined);
 assert.equal(routeConditions({mode:'shared',endpoint:'default'},measuredPlayback,health).assessment,'partial');
 assert.deepEqual(measuredPlayback,before);
});
test('assessment renders translated scope, clears when inactive and stays out of Shared and compact views',async()=>{
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});
  const t=i18n.t.bind(i18n),SignalPath=compile(t);
  const render=(props={})=>renderToStaticMarkup(React.createElement(SignalPath,{route,status:measuredPlayback,health,...props}));
  for(const horizontal of [false,true])for(const kind of ['local','plex']){
   const html=render({horizontal,status:{...measuredPlayback,current_media:{...playback.current_media,kind}}});
   assert.ok(html.includes('data-route-assessment="compatible"'));
   assert.ok(html.includes(t('signalPath.assessment.compatible')));
   assert.ok(html.includes(t('signalPath.assessment.help')));
   assert.ok(html.includes(t('signalPath.observation.notVerified'))||html.includes(t('signalPath.seal.scope')));
   assert.doesNotMatch(html,/signalPath\.|data-bitperfect/);
  }
  assert.ok(render({status:{...measuredPlayback,processing:{...processing,mpd_volume:50}}}).includes('data-route-assessment="attention"'));
  assert.ok(render({status:{...measuredPlayback,engine_evidence:undefined}}).includes('data-route-assessment="compatible"'));
  for(const props of [{route:{mode:'shared',endpoint:'default'}},{status:{...measuredPlayback,state:'pause'}},{health:{state:'unavailable'}}])assert.doesNotMatch(render(props),/data-route-assessment/);
 }
});

test('USB model evidence is scoped to the selected observed route and never authenticates a unit',()=>{
 assert.equal(row(measuredPlayback,'device').message,'deviceObserved');
 assert.equal(row({...measuredPlayback,output_observation:{...observation,card:7,usb_device:measuredPlayback.output_observation.usb_device}},'device').message,'deviceObserved');
 assert.equal(row(measuredPlayback,'device',{mode:'direct',endpoint:'hw:Other,0'}).message,'notChecked');
 assert.equal(row(measuredPlayback,'device',{mode:'shared',endpoint:'default'}).message,'notChecked');
 assert.equal(row({...measuredPlayback,output_observation:{...observation,usb_device:{vendor_id:'1234',product_id:'5678',model:'Other DAC'}}},'device').message,'deviceObserved');
});

test('kernel information cannot block live assessment; only exact matches attach historical evidence',()=>{
 assert.equal(row(measuredPlayback,'kernel').message,'kernelObserved');
 for(const kernel_release of [undefined,null,'','6.12.108-1-MANJARO\n','invalid version']){
  const status={...measuredPlayback,output_observation:{...measuredPlayback.output_observation,kernel_release}};
  assert.equal(row(status,'kernel').message,'notChecked');
  assert.equal(routeConditions(route,status,health).assessment,'compatible');
 }
 for(const kernel_release of ['7.1.0-1-MANJARO','6.12.109-1-MANJARO']){
  const status={...measuredPlayback,output_observation:{...measuredPlayback.output_observation,kernel_release}};
  assert.equal(row(status,'kernel').message,'kernelObserved');
  assert.equal(routeConditions(route,status,health).assessment,'compatible');
 }
 assert.equal(row(measuredPlayback,'kernel',{mode:'shared',endpoint:'default'}).message,'notChecked');
 assert.equal(row(measuredPlayback,'kernel',{mode:'direct',endpoint:'hw:Other,0'}).message,'notChecked');
});

test('a completed profile links historical proof and gives actionable reasons without certifying current samples',()=>{
 const full=routeConditions(route,measuredPlayback,health);
 assert.equal(full.summary,'preserved');assert.equal(full.profile.id,'usb-e70-pcm24-96-20261017');assert.deepEqual(full.issues,[]);
 assert.equal(full.profile.measured.frames,192000);assert.equal(full.profile.measured.tracks,2);
 const volume=routeConditions(route,{...measuredPlayback,processing:{...processing,mpd_volume:50}},health);
 assert.equal(volume.issues[0].label,'volume');assert.equal(volume.issues[0].message,'softwareVolume');assert.equal(volume.profile,undefined);
 const other={...measuredPlayback,output_observation:{...measuredPlayback.output_observation,usb_device:{vendor_id:'1234',product_id:'5678',model:'Other DAC'}}};
 const result=routeConditions(route,other,health);assert.equal(result.assessment,'compatible');
 assert.deepEqual(result.issues,[]);assert.equal(result.profile,undefined);
 const changedVolume=routeConditions(route,{...measuredPlayback,volume:{...playback.volume,value:50}},health);
 assert.equal(changedVolume.issues[0].message,'volumeChanged');assert.equal(changedVolume.profile,undefined);
 for(const usb_device of [undefined,{state:'model_matched',profile_id:'e70-usb-152a-8750'},{vendor_id:'152a\n',product_id:'8750',model:'E70'},{vendor_id:'152a',product_id:'8750',model:'E70\n'}]){
  const result=routeConditions(route,{...measuredPlayback,output_observation:{...measuredPlayback.output_observation,usb_device}},health);
  assert.equal(result.assessment,'compatible');assert.equal(result.profile,undefined);
 }
 const selected={mode:'direct',endpoint:'hw:CARD=Test-DAC,DEV=0'};
 const renamed={...measuredPlayback,output_observation:{...measuredPlayback.output_observation,configured_endpoint:selected.endpoint}};
 assert.equal(routeConditions(selected,renamed,health).assessment,'compatible');
});
test('integrated UI separates live readiness from reference evidence for other DACs in both languages',async()=>{
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});
  const t=i18n.t.bind(i18n),SignalPath=compile(t);
  const render=status=>renderToStaticMarkup(React.createElement(SignalPath,{route,status,health,horizontal:true}));
  const full=render(measuredPlayback);
  assert.equal((full.match(/data-route-assessment=/g)||[]).length,1);
  assert.ok(full.includes('data-route-profile="usb-e70-pcm24-96-20261017"'));
  assert.ok(full.includes(t('signalPath.evidence.reference',{frames:192000,tracks:2})));
  assert.doesNotMatch(full,/data-route-reason|signalPath\.|data-bitperfect/);
  const other=render({...measuredPlayback,output_observation:{...measuredPlayback.output_observation,usb_device:{vendor_id:'1234',product_id:'5678',model:'Other DAC'}}});
  assert.ok(other.includes(t('signalPath.assessment.compatible')));assert.doesNotMatch(other,/data-route-reason|data-route-profile|Fora do perfil|Outside the tested profile/);assert.match(other,/Other DAC/);
  const volume=render({...measuredPlayback,processing:{...processing,mpd_volume:50}});
  assert.match(volume,/data-route-reason/);assert.ok(volume.includes(t('signalPath.conditions.value.softwareVolume',{value:50})));assert.doesNotMatch(volume,/data-route-profile/);
 }
});

test('any DAC model and accepted integer PCM format use the same live conditions',()=>{
 const devices=[
  {vendor_id:'152a',product_id:'8750',model:'E70'},
  {vendor_id:'2972',product_id:'0120',model:'FIIO K13 R2R'},
  {vendor_id:'1234',product_id:'5678',model:'Uncatalogued DAC'},
  undefined,
 ];
 for(const usb_device of devices)for(const [rate,bits,channels,format] of [
  [44100,16,2,'S16_LE'],[48000,16,1,'S32_LE'],[96000,24,2,'S24_3LE'],
  [192000,24,2,'S32_LE'],[384000,32,2,'S32_BE'],[48000,24,6,'S24_LE'],
 ]){
  const status={...measuredPlayback,engine_evidence:undefined,audio_format:`${rate}:${bits}:${channels}`,output_observation:{...observation,rate,channels,format,usb_device,kernel_release:'other-kernel'}};
  const result=routeConditions(route,status,health);
  assert.equal(result.assessment,'compatible',JSON.stringify({usb_device,rate,bits,channels,format}));
  assert.equal(result.profile,undefined);assert.deepEqual(result.issues,[]);
 }
 const fiio={...measuredPlayback,output_observation:{...measuredPlayback.output_observation,usb_device:devices[1]}};
 const ready=routeConditions(route,fiio,health);assert.equal(ready.assessment,'compatible');assert.equal(ready.profile,undefined);
 const altered=routeConditions(route,{...fiio,processing:{...processing,mpd_volume:50}},health);
 assert.equal(altered.assessment,'attention');assert.equal(altered.issues[0].label,'volume');
 const resampled=routeConditions(route,{...fiio,output_observation:{...fiio.output_observation,rate:48000}},health);
 assert.equal(resampled.assessment,'attention');assert.equal(resampled.issues[0].message,'rateDifferent');
});

test('bit-perfect route seal works for different DACs and withdraws for incomplete or altered conditions',()=>{
 for(const usb_device of [{vendor_id:'152a',product_id:'8750',model:'E70'},{vendor_id:'2972',product_id:'0120',model:'FIIO K13 R2R'},undefined]){
  const status={...measuredPlayback,output_observation:{...measuredPlayback.output_observation,usb_device,kernel_release:'other-kernel'}};
  assert.equal(routeConditions(route,status,health).bitPerfect,true);
  assert.equal(routeConditions(route,{...status,engine_evidence:undefined},health).bitPerfect,false);
  assert.equal(routeConditions(route,{...status,processing:{...processing,mpd_volume:50}},health).bitPerfect,false);
  assert.equal(routeConditions(route,{...status,processing:{...processing,replay_gain:{state:'track'}}},health).bitPerfect,false);
  assert.equal(routeConditions(route,{...status,audio_format:'dsd64:2'},health).bitPerfect,false);
  assert.equal(routeConditions(route,{...status,state:'pause'},health).bitPerfect,undefined);
  assert.equal(routeConditions({mode:'shared',endpoint:'default'},status,health).bitPerfect,false);
 }
});
test('main view restores audio data and groups seal with help; route diagnostics remain in the closed dialog',async()=>{
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});
  const t=i18n.t.bind(i18n),Main=compile(t,'SignalPath');
  for(const compact of [false,true]){
   const html=renderToStaticMarkup(React.createElement(Main,{route,status:measuredPlayback,health,compact}));
   assert.match(html,/data-bitperfect="route"/);assert.match(html,/Bit-perfect/);assert.match(html,/aria-haspopup="dialog"/);assert.match(html,/aria-expanded="false"/);
   assert.match(html,/96 kHz/); assert.match(html,/24-bit/);
   assert.doesNotMatch(html,/E70|MANJARO|data-route-conditions|data-route-profile|data-alsa-observation|<dialog/);
   if(!compact) { assert.match(html,/np-signal-stages/); assert.match(html,/np-signal-format/); }
   const tree=Main({route,status:measuredPlayback,health,compact});
   const group=nodes(tree).find(n=>n.props&&"data-signal-badge" in n.props);
   assert.ok(group); assert.match(group.props.className,/inline-flex/); assert.doesNotMatch(group.props.className,/justify-between|flex-1/);
   assert.ok(nodes(group).some(n=>n.props?.["data-bitperfect"]==="route"));
   assert.ok(nodes(group).some(n=>n.props?.openLabel===t("signalPath.modal.open")));
   assert.equal((html.match(/<button/g)||[]).length,1);
   const altered=renderToStaticMarkup(React.createElement(Main,{route,status:{...measuredPlayback,processing:{...processing,mpd_volume:50}},health,compact}));
   assert.doesNotMatch(altered,/data-bitperfect/); assert.match(altered,/96 kHz/);assert.ok(altered.includes(t('signalPath.assessment.attention')));
  }
 }
});
test('help dialog opens on click, enters native top layer, stops playback keys, closes on Escape/backdrop and restores focus',()=>{
 let open=false,refIndex=0,effects=[],shown=0,closed=0,focused=0;
 const element={open:false,showModal(){this.open=true;shown++;},close(){this.open=false;closed++;}};
 const refs=[{current:element},{current:{focus(){focused++;}}}];
 const hooks={...React,useState:()=>[open,next=>{open=next;}],useRef:()=>refs[refIndex++],useEffect:fn=>effects.push(fn)};
 const Info=compile(key=>key,'SignalPathInfo',hooks);
 const render=()=>{refIndex=0;effects=[];return Info({title:'Technical details',openLabel:'Help',closeLabel:'Close',children:React.createElement('p',{},'Technical content')});};
 let tree=render();assert.ok(!nodes(tree).some(n=>n.type==='dialog'));effects[0]();
 let stopped=0;const trigger=nodes(tree).find(n=>n.type==='button');
 trigger.props.onKeyDown({key:' ',stopPropagation(){stopped++;}});trigger.props.onClick({stopPropagation(){stopped++;}});assert.equal(stopped,2);assert.equal(open,true);
 tree=render();const dialog=nodes(tree).find(n=>n.type==='dialog');assert.ok(dialog);assert.equal(dialog.props['aria-modal'],'true');const cleanup=effects[0]();assert.equal(shown,1);
 for(const key of [' ','Enter','Escape','ArrowRight'])dialog.props.onKeyDown({key,stopPropagation(){stopped++;}});
 let prevented=0;dialog.props.onCancel({preventDefault(){prevented++;}});assert.equal(prevented,1);assert.equal(open,false);cleanup();assert.equal(closed,1);assert.equal(focused,1);
 open=true;tree=render();const reopened=nodes(tree).find(n=>n.type==='dialog');const target={};reopened.props.onClick({target:{},currentTarget:target,stopPropagation(){}});assert.equal(open,true);
 reopened.props.onClick({target,currentTarget:target,stopPropagation(){}});assert.equal(open,false);
 open=true;tree=render();const close=nodes(tree).find(n=>n.type==='button'&&n.props.autoFocus);close.props.onClick();assert.equal(open,false);
});

const dsdEngine={state:'identified',evidence_id:'mpd-route-20261017',scope:'pcm_dsd_route',pcm_eof_flush:true};
function dsdStatus(level=64,kind='native_dsd',format='DSD_U32_BE') {
 const bitRate=44100*level,word=kind==='dop'?16:format==='DSD_U8'?8:format.includes('16')?16:32;
 const rate=bitRate/word;
 return {...measuredPlayback,audio_format:`dsd${level}:2`,engine_evidence:dsdEngine,
  output_observation:{...observation,format,rate},
  processing:{...processing,transport:{kind,source_rate:bitRate/8,output_rate:rate,channels:2,epoch:1}}};
}
test('native DSD and DoP require negotiated transport and preserve DSD bit rate across all container sizes',()=>{
 for(const level of [64,128,256,512,1024])for(const [kind,format] of [
  ['native_dsd','DSD_U8'],['native_dsd','DSD_U16_LE'],['native_dsd','DSD_U16_BE'],['native_dsd','DSD_U32_LE'],['native_dsd','DSD_U32_BE'],
  ['dop','S24_LE'],['dop','S24_BE'],['dop','S24_3LE'],['dop','S24_3BE'],['dop','S32_LE'],['dop','S32_BE']]) {
  const status=dsdStatus(level,kind,format),result=routeConditions(route,status,health);
  assert.equal(result.bitPerfect,true,JSON.stringify({level,kind,format,result}));
  assert.equal(result.summary,'preserved');assert.equal(result.transport,kind);assert.equal(result.profile,undefined);
  assert.equal(result.rows.find(r=>r.label==='rate').message,'dsdRateEqual');
  const long={...status,audio_format:`${44100*level/8}:dsd:2`};assert.equal(routeConditions(route,long,health).bitPerfect,true);
 }
});
test('DSD conversion, stale transport, old engines, processing and incompatible carriers never earn a seal',()=>{
 for(const kind of ['native_dsd','dop']){
  const good=dsdStatus(64,kind,kind==='dop'?'S32_LE':'DSD_U32_BE');
  const changes=[
   {engine_evidence:undefined},{engine_evidence:measuredPlayback.engine_evidence},
   {processing:{...good.processing,transport:null}},
   ...[{kind:'pcm'},{kind:'unknown'},{source_rate:705600},{source_rate:'352800'},{output_rate:44100},{channels:1},{epoch:0},{epoch:NaN}].map(t=>({processing:{...good.processing,transport:{...good.processing.transport,...t}}})),
   {processing:{...good.processing,mpd_volume:50}},{volume:{...good.volume,value:50}},
   {processing:{...good.processing,replay_gain:{state:'album'}}},{processing:{...good.processing,crossfade_seconds:2}},
   {processing:{...good.processing,mixramp:'possible'}},
   {output_observation:{...good.output_observation,channels:1}},
   {output_observation:{...good.output_observation,format:'S16_LE'}},
   {state:'pause'},{state:'stop'},{audio_format:'96000:24:2'},
  ];
  for(const change of changes)assert.notEqual(routeConditions(route,{...good,...change},health).bitPerfect,true,JSON.stringify({kind,change}));
  const wrongRate={...good,output_observation:{...good.output_observation,rate:good.output_observation.rate*2},processing:{...good.processing,transport:{...good.processing.transport,output_rate:good.output_observation.rate*2}}};
  assert.equal(routeConditions(route,wrongRate,health).assessment,'attention');
  assert.equal(routeConditions({mode:'shared',endpoint:'default'},good,health).bitPerfect,false);
 }
});
test('PCM seal follows decoded signal conditions for all accepted depths and never relies on codec metadata',()=>{
 for(const bits of [8,16,24,32])for(const kind of ['local','plex']){
  const status={...measuredPlayback,audio_format:`44100:${bits}:2`,engine_evidence:dsdEngine,current_media:{kind},output_observation:{...observation,rate:44100,format:'S32_LE'}};
  assert.equal(routeConditions(route,status,health).bitPerfect,true);
 }
});
test('DSD seal and negotiated path are visible while detailed transport verification stays in the help modal',async()=>{
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});
  const t=i18n.t.bind(i18n),Main=compile(t,'SignalPath'),Details=compile(t);
  for(const kind of ['native_dsd','dop'])for(const compact of [false,true]){
   const status=dsdStatus(128,kind,kind==='dop'?'S32_LE':'DSD_U32_BE');
   const html=renderToStaticMarkup(React.createElement(Main,{route,status,health,compact}));
   assert.match(html,/data-bitperfect="route"/);assert.match(html,/DSD128/);assert.match(html,/aria-haspopup="dialog"/);
   assert.doesNotMatch(html,/data-route-conditions|data-alsa-observation|signalPath\./);
   if(!compact)assert.match(html,new RegExp(`data-signal-transport="${kind}"`));
   const detail=renderToStaticMarkup(React.createElement(Details,{route,status,health}));
   assert.ok(detail.includes(t(`signalPath.conditions.value.${kind==='dop'?'dop':'nativeDsd'}`)));assert.doesNotMatch(detail,/signalPath\./);
  }
 }
});

test('Shared omits the route verdict in both players but retains help, mode and audio metadata',async()=>{
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});const t=i18n.t.bind(i18n),Main=compile(t,'SignalPath');
  for(const compact of [true,false])for(const gain of ['off','album']){
   const html=renderToStaticMarkup(React.createElement(Main,{route:{mode:'shared',endpoint:'default'},status:{...playback,processing:{...processing,replay_gain:{state:gain}}},health,compact}));
   assert.match(html,/aria-haspopup="dialog"/);assert.match(html,/96 kHz/);assert.ok(html.includes(t('signalPath.shared')));
   assert.doesNotMatch(html,/data-route-assessment|data-bitperfect/);for(const key of ['partial','attention','compatible'])assert.ok(!html.includes(t(`signalPath.assessment.${key}`)));
  }
 }
});
test('verified DSD software gain bypass preserves the seal at nonzero settings, while unconfirmed zero volume and inconsistent readings remove it',()=>{
 for(const kind of ['native_dsd','dop'])for(const value of [1,50,100]){
  const base=dsdStatus(64,kind,kind==='dop'?'S32_LE':'DSD_U32_BE');const status={...base,volume:{...base.volume,value},processing:{...base.processing,mpd_volume:value}};
  assert.equal(routeConditions(route,status,health).bitPerfect,true);assert.equal(row(status,'volume').message,'dsdSoftwareBypass');
  const mute={...status,volume:{...status.volume,value:0},processing:{...status.processing,mpd_volume:0}};assert.equal(routeConditions(route,mute,health).bitPerfect,false);assert.equal(row(mute,'volume').message,'dsdMuted');
  assert.equal(routeConditions(route,{...status,volume:{...status.volume,value:value===100?50:100}},health).bitPerfect,false);
 }
 const pcm={...measuredPlayback,volume:{...measuredPlayback.volume,value:50},processing:{...processing,mpd_volume:50}};assert.equal(routeConditions(route,pcm,health).bitPerfect,false);
});

test('confirmed mute retains the Direct route seal for PCM, native DSD and DoP without hiding other problems',async()=>{
 const cases=[measuredPlayback,dsdStatus(),dsdStatus(64,'dop','S32_LE')];
 for(const base of cases){
  const muted={...base,volume:{...base.volume,value:0,muted:true},processing:{...base.processing,mpd_volume:0}};
  assert.equal(routeConditions(route,muted,health).bitPerfect,true);assert.equal(row(muted,'volume').message,'softwareMuted');
  for(const change of [
   {engine_evidence:undefined}, {output_observation:{state:'unverified',reason:'changed'}},
   {processing:{...muted.processing,replay_gain:{state:'album'}}},
   {processing:{...muted.processing,crossfade_seconds:1}},
   {processing:{...muted.processing,mpd_volume:50}},
   {volume:{...muted.volume,value:50}}, {volume:{...muted.volume,muted:false}},
  ])assert.equal(routeConditions(route,{...muted,...change},health).bitPerfect,false,JSON.stringify(change));
  const restored={...base,volume:{...base.volume,value:50,muted:false},processing:{...base.processing,mpd_volume:50}};
  assert.equal(routeConditions(route,restored,health).bitPerfect,base.audio_format.includes('dsd'));
  assert.equal(routeConditions({mode:'shared',endpoint:'default'},muted,health).bitPerfect,false);
 }
 for(const lng of ['en','pt-BR']){
  const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});const t=i18n.t.bind(i18n),Main=compile(t,'SignalPath'),Details=compile(t);
  for(const base of cases)for(const compact of [false,true]){
   const status={...base,volume:{...base.volume,value:0,muted:true},processing:{...base.processing,mpd_volume:0}};
   const html=renderToStaticMarkup(React.createElement(Main,{route,status,health,compact}));assert.match(html,/data-bitperfect="route"/);assert.doesNotMatch(html,/data-route-assessment="attention"/);
   const details=renderToStaticMarkup(React.createElement(Details,{route,status,health}));assert.ok(details.includes(t('signalPath.conditions.value.softwareMuted')));assert.doesNotMatch(details,/signalPath\./);
  }
 }
});
