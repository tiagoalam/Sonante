import * as numbers from '../src/components/eqNumbers.ts';
import * as equalizerError from '../src/utils/equalizerError.ts';
import * as equalizerAvailability from '../src/utils/equalizerAvailability.ts';
import * as audioExperience from '../src/utils/audioExperience.ts';
import test from 'node:test';import assert from 'node:assert/strict';import {readFileSync,writeFileSync,readdirSync} from 'node:fs';import ts from 'typescript';import React from 'react';import * as jsx from 'react/jsx-runtime';import * as icons from 'lucide-react';import i18next from 'i18next';import {renderToStaticMarkup} from 'react-dom/server';import * as math from '../src/utils/equalizer.ts';import * as graphMath from '../src/utils/equalizerGraph.ts';
const i18n=i18next.createInstance();await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(l=>[l,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${l}.json`,import.meta.url)))}]))});
const compile=(file,deps)=>{const code=ts.transpileModule(readFileSync(new URL(`../src/${file}`,import.meta.url),'utf8'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const ex={};new Function('require','exports',code)(n=>{if(n==='./eqNumbers')return numbers;if(n==='../utils/equalizerGraph')return graphMath;if(n==='../utils/equalizerAvailability')return equalizerAvailability;if(n==='./EqHelp')return {EqHelp:({title})=>React.createElement('button',{'aria-label':title},'?')};if(n==='./OpraCatalogBrowser')return {OpraCatalogBrowser:()=>null};if(n==='./EqNumberInput')return compile('components/EqNumberInput.tsx',{'react':React,'react/jsx-runtime':jsx,'./eqNumbers':numbers});if(n==='./AudioModeSelector')return compile('components/AudioModeSelector.tsx',{'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'lucide-react':icons,'react/jsx-runtime':jsx});if(n==='../utils/audioExperience')return audioExperience;assert.ok(n in deps,n);return deps[n]},ex);return ex;};
const off={in_path:false,revision:0,phase:'off',applied:null,error:null,pcm_rate:48000,pcm_channels:2,verified:false};const catalog=()=>({config:{schema_version:1,enabled:false,active_preset:{kind:'built_in',id:'flat'},user_presets:[]},revision:'fixture',built_ins:[math.flatPreset()],runtime:off,shared:true,runtime_available:true});
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
function host(service,props={}){let states=[],refs=[],effects=[],si=0,ri=0;const cleanups=[];const hooks={...React,default:React,useState:v=>{const i=si++;if(!(i in states))states[i]=typeof v==='function'?v():v;return[states[i],n=>states[i]=typeof n==='function'?n(states[i]):n]},useRef:v=>refs[ri++]??={current:v},useMemo:f=>f(),useCallback:f=>f,useEffect:f=>effects.push(f)};const graph=compile('components/EqGraph.tsx',{'react':{...React,useMemo:f=>f(),useRef:v=>({current:v})},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../utils/equalizer':math});const {EqualizerView}=compile('components/EqualizerView.tsx',{'react':hooks,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../services/equalizer':{equalizerService:service},'../utils/equalizer':math,'../utils/equalizerError':equalizerError,'./equalizer.css':{},'./EqGraph':graph,'./EqComparison':compile('components/EqComparison.tsx',{'react':React,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../services/equalizer':{equalizerService:service}}),'./EqLibrary':compile('components/EqLibrary.tsx',{'react':React,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../services/equalizer':{equalizerService:service},'../utils/equalizer':math,'./EqGraph':graph})});let mounted=false;return{render(extra={}){si=ri=0;effects=[];const tree=EqualizerView({shared:true,available:true,runtime:off,onConfigure(){},...props,...extra});if(!mounted){mounted=true;for(const f of effects){const cleanup=f();if(typeof cleanup==='function')cleanups.push(cleanup)}}return tree},unmount(){for(const f of cleanups)f()},states};}
const flush=async()=>{for(let i=0;i<20;i++)await Promise.resolve()};const by=(tree,text)=>nodes(tree,n=>n.type==='button'&&n.props.children===text)[0];
test('graph uses an adaptive dB scale, shares pointer and point coordinates, and honestly expands protected gain',()=>{
 const ref={current:null},changes=[],added=[];const {EqGraph}=compile('components/EqGraph.tsx',{'react':{useRef:()=>ref,useMemo:f=>f(),useState:v=>[v,()=>{}],useEffect:()=>{}},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../utils/equalizer':math});
 const p=math.flatPreset();p.bands[0].enabled=true;p.bands[0].gain_db=12;
 const props={preset:p,budget:math.protection(p),selected:null,onSelect(){},onChange:(...v)=>changes.push(v),onAdd:(...v)=>added.push(v)};
 let tree=EqGraph(props);let texts=nodes(tree,n=>n.type==='text').map(n=>n.props.children).flat().join(' ');assert.match(texts,/-15/);assert.doesNotMatch(texts,/-24/);let circle=nodes(tree,n=>n.type==='circle')[0];assert.ok(Math.abs(circle.props.cy-54.8)<1e-8);
 const svg=nodes(tree,n=>n.type==='svg')[0],target={getBoundingClientRect:()=>({left:0,top:0,width:1000,height:350})};svg.props.onDoubleClick({clientX:500,clientY:162,currentTarget:target,target});assert.equal(added.at(-1)[1],0);svg.props.onDoubleClick({clientX:500,clientY:28,currentTarget:target,target});assert.equal(added.at(-1)[1],15);
 circle.props.onPointerDown({button:0,pointerId:1,preventDefault(){},currentTarget:{setPointerCapture(){},focus(){}}});svg.props.onPointerMove({clientX:500,clientY:269.2,currentTarget:target});assert.ok(Math.abs(changes.at(-1)[1].gain_db+12)<1e-8);svg.props.onPointerCancel();assert.deepEqual(changes.at(-1)[1],p.bands[0]);
 const flat=math.flatPreset();tree=EqGraph({...props,preset:flat,budget:{...math.protection(flat),effective_preamp_db:-23},showGain:true});texts=nodes(tree,n=>n.type==='text').map(n=>n.props.children).flat().join(' ');assert.match(texts,/-30/);assert.match(renderToStaticMarkup(tree),/expanded/);const line=nodes(tree,n=>n.type==='path'&&n.props.strokeDasharray)[0];const ys=line.props.d.split(' ').map(v=>Number(v.split(',')[1]));assert.ok(ys.every(y=>Math.abs(y-(162+23*268/60))<1e-8));assert.equal(line.props.strokeWidth,'2.6');
 const modest=math.flatPreset();modest.bands[0].enabled=true;modest.bands[0].gain_db=3;tree=EqGraph({...props,preset:modest,budget:math.protection(modest)});texts=nodes(tree,n=>n.type==='text').map(n=>n.props.children).flat().join(' ');assert.match(texts,/-4/);assert.match(texts,/-2\.7/);assert.match(texts,/-1\.3/);assert.doesNotMatch(texts,/-15/);assert.equal(nodes(tree,n=>n.type==='text'&&n.props.x===41&&n.props.textAnchor==='end').length,7);
 const peak=nodes(tree,n=>n.type==='circle'&&n.props['data-eq-point'])[0];assert.equal(peak.props.r,9);assert.equal(peak.props.cy,61.5);const requested=nodes(tree,n=>n.type==='path'&&n.props.stroke==='#f0b429')[0];assert.equal(requested.props.strokeWidth,'3.2');
 const stacked=math.flatPreset();for(const b of stacked.bands){b.enabled=true;b.frequency_hz=1000;b.gain_db=12;}tree=EqGraph({...props,preset:stacked,budget:null});assert.match(renderToStaticMarkup(tree),/expanded/);for(const path of nodes(tree,n=>n.type==='path'))assert.ok(path.props.d.split(' ').every(v=>{const y=Number(v.split(',')[1]);return y>=28-1e-7&&y<=296+1e-7}));
});
test('graph safely maps per-filter gains through ±15 dB and expands for stacked combined response',()=>{
 for(const gain of [-15,-12,0,12,15]){
  const preset=math.flatPreset();preset.bands[0].enabled=true;preset.bands[0].gain_db=gain;
  assert.equal(math.validDraft(preset),true);const sampled=graphMath.sampleEqualizerGraph(preset,null,false);
  const range=graphMath.equalizerGraphRange([...sampled.individual,sampled.requested,preset.bands.filter(b=>b.enabled).map(b=>b.gain_db)]);
  assert.ok(range>=Math.abs(gain));const y=graphMath.equalizerGraphY(gain,28,268,range);
  assert.ok(y>=28&&y<=296);assert.ok(Math.abs(graphMath.equalizerGraphGain(y,28,268,range)-gain)<1e-8);
  const path=graphMath.equalizerGraphPath(sampled.requested,48,904,28,268,range);
  assert.ok(path.length>0&&!/NaN|Infinity/.test(path));
 }
 const stacked=math.flatPreset();for(const band of stacked.bands){band.enabled=true;band.frequency_hz=1000;band.gain_db=15;}
 const sampled=graphMath.sampleEqualizerGraph(stacked,math.protection(stacked)?.effective_preamp_db??null,true);
 const individualMaximum=Math.max(...sampled.individual.flatMap(curve=>curve??[]));
 const combinedMaximum=Math.max(...sampled.requested);
 assert.ok(individualMaximum<=15+1e-7);assert.ok(combinedMaximum>15);
 const range=graphMath.equalizerGraphRange([...sampled.individual,sampled.requested,...(sampled.combined?[sampled.combined]:[])]);
 assert.ok(range>15);for(const curve of [...sampled.individual,sampled.requested,...(sampled.combined?[sampled.combined]:[])])if(curve){
  const path=graphMath.equalizerGraphPath(curve,48,904,28,268,range);assert.ok(path.length>0&&!/NaN|Infinity/.test(path));
  for(const pair of path.replace(/[ML]/g,' ').trim().split(/\s+/)){const [x,y]=pair.split(',').map(Number);assert.ok(Number.isFinite(x)&&Number.isFinite(y));assert.ok(y>=28-1e-7&&y<=296+1e-7);}
 }
});
test('Style and Mood choices render translated groups, retain homonymous users and only select on intent',async()=>{
 const legacy=JSON.parse(readFileSync(new URL('../src-tauri/src/equalizer_builtin_legacy_v1.json',import.meta.url),'utf8'));
 const fresh=JSON.parse(readFileSync(new URL('fixtures/equalizer-collection.json',import.meta.url),'utf8')).map(c=>c.preset);
 const data=catalog();data.built_ins=[...legacy,...fresh];data.config.user_presets=[{...math.flatPreset(),id:'user_rock',name:'Rock'}];
 data.builtin_groups=[{id:'reference',preset_ids:['flat']},{id:'styles',preset_ids:fresh.filter(p=>p.id.startsWith('style_')).map(p=>p.id)},{id:'moods',preset_ids:[...fresh.filter(p=>p.id.startsWith('mood_')).map(p=>p.id),'voices_v2']},{id:'tonal',preset_ids:['gentle_bass_v2','smooth_v2','light_loudness_v2']},{id:'legacy',preset_ids:['style_rock']}];
 const calls=[];const h=host({get:async()=>data,edit:async(revision,edit)=>{calls.push(edit);assert.equal(edit.action,'select');data.config.active_preset=edit.reference;return data},apply:async()=>{throw Error('Selection must not apply')}});h.render();await flush();
 for(const lang of ['en','pt-BR']){await i18n.changeLanguage(lang);let tree=h.render();let library=nodes(tree,n=>n.type?.name==='EqLibrary')[0];assert.ok(library,'the single editor keeps its library at the bottom');assert.equal(library.props.catalog,data);assert.equal(library.props.draft,h.states[1]);const select=nodes(tree,n=>n.type==='select'&&n.props['aria-label']===i18n.t('equalizer.preset'))[0];assert.ok(select,'preset selection remains beside the editor controls');const groups=nodes(tree,n=>n.type==='optgroup');assert.deepEqual(groups.map(g=>g.props.label),['reference','styles','moods','tonal','legacy','user'].map(id=>i18n.t('equalizer.groups.'+id)));const options=nodes(tree,n=>n.type==='option'&&n.props.value?.startsWith('{'));assert.equal(options.length,17);assert.equal(new Set(options.map(o=>o.props.value)).size,17);assert.equal(options.filter(o=>o.props.children==='Rock').length,3);
  for(const reference of [{kind:'built_in',id:'style_rock_v2'},{kind:'built_in',id:'mood_cozy_v2'},{kind:'built_in',id:'style_rock'},{kind:'user',id:'user_rock'}]){select.props.onChange({target:{value:JSON.stringify(reference)}});await flush();tree=h.render();assert.deepEqual(data.config.active_preset,reference);assert.equal(h.states[1].id,reference.id);assert.equal(h.states[5],null);assert.equal(nodes(tree,n=>n.props?.['data-eq-preset-description']!==undefined).length,reference.kind==='built_in'?1:0);}
 }
 assert.equal(calls.length,8);assert.ok(calls.every(c=>c.action==='select'));h.unmount();await i18n.changeLanguage('en');
});
test('natural factory collection preserves identities and moderate shapes with the exact Rust budget',()=>{
 const cases=JSON.parse(readFileSync(new URL('fixtures/equalizer-collection.json',import.meta.url),'utf8'));assert.equal(cases.length,14);const signatures=[];
 for(const c of cases){const p=c.preset;assert.ok(math.validDraft(p));assert.equal(p.bands.length,10);assert.ok(p.bands.filter(b=>b.enabled).length<=10);const budget=math.protection(p);assert.ok(budget);for(const key of ['effective_preamp_db','filter_bound_db','attenuation_db'])assert.ok(Math.abs(budget[key]-c.protection[key])<1e-7,`${p.id} ${key}`);
 const shape={...p,preamp_db:0};const curve=Array.from({length:1001},(_,i)=>math.responseDb(shape,20*1000**(i/1000)));assert.ok(curve.every(Number.isFinite));assert.ok(Math.max(...curve)<=5&&Math.min(...curve)>=-5,p.id);assert.equal(p.preamp_db,0);assert.ok(budget.effective_preamp_db+budget.filter_bound_db+budget.resampler_bound_db<=-1+1e-7,p.id+' protected output budget');signatures.push(curve.filter((_,i)=>i%50===0));
 for(let i=0;i<10;i++){const b=math.coefficients(p.bands[i],48000),got=[b[0]/b[3],b[1]/b[3],b[2]/b[3],b[4]/b[3],b[5]/b[3]];for(let k=0;k<5;k++)assert.ok(Math.abs(got[k]-c.coefficients[i][k])<1e-12);}
 }for(let i=0;i<signatures.length;i++)for(let j=i+1;j<signatures.length;j++)assert.ok(Math.max(...signatures[i].map((v,k)=>Math.abs(v-signatures[j][k])))>.2,`${cases[i].preset.id}/${cases[j].preset.id}`);
});
test('curve: unit Flat, peak center, preamp, disabled band, shelf ends and safe bounds',()=>{const p=math.flatPreset();assert.equal(p.bands.length,10);assert.ok(math.validDraft(p));for(const hz of [20,997,20000])assert.ok(Math.abs(math.responseDb(p,hz))<1e-8);p.bands[0]={...p.bands[0],enabled:true,frequency_hz:997,gain_db:6};assert.ok(Math.abs(math.responseDb(p,997)-6)<1e-8);p.preamp_db=-6;assert.ok(Math.abs(math.responseDb(p,997))<1e-8);p.bands[0].enabled=false;assert.ok(Math.abs(math.responseDb(p,997)+6)<1e-8);p.preamp_db=0;p.bands[0].enabled=true;p.bands[0].filter_type='low_shelf';assert.ok(math.responseDb(p,20)>5.9);assert.ok(Math.abs(math.responseDb(p,20000))<.01);p.bands[0].filter_type='high_shelf';assert.ok(math.responseDb(p,20000)>5.9);p.bands[0].q=NaN;assert.equal(math.validDraft(p),false);p.bands.pop();assert.equal(math.validDraft(p),false);});
test('EqualizerView exposes ±15 dB, associates inline gain errors outside the stable band row, and preserves the independent preamp range',async()=>{
 const h=host({get:async()=>catalog()});h.render();await flush();
 const add=tree=>nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.addBand')))[0];
 add(h.render()).props.onClick();let tree=h.render();
 const gain=nodes(tree,n=>typeof n.type==='function'&&n.props['aria-label']===i18n.t('equalizer.gain'))[0];
 const preamp=nodes(tree,n=>typeof n.type==='function'&&n.props['aria-label']===i18n.t('equalizer.preampValue'))[0];
 assert.ok(gain&&preamp);assert.equal(gain.props.min,-15);assert.equal(gain.props.max,15);
 assert.equal(preamp.props.min,-24);assert.equal(preamp.props.max,12);
 for(const language of ['en','pt-BR']){
  await i18n.changeLanguage(language);tree=h.render();let control=nodes(tree,n=>typeof n.type==='function'&&n.props['aria-label']===i18n.t('equalizer.gain'))[0];
  assert.equal(control.props.min,-15);assert.equal(control.props.max,15);assert.equal(control.props['aria-describedby'],undefined);assert.equal(typeof control.props.onValidityChange,'function');
  control.props.onValidityChange(true);tree=h.render();control=nodes(tree,n=>typeof n.type==='function'&&n.props['aria-label']===i18n.t('equalizer.gain'))[0];
  const error=nodes(tree,n=>n.props['data-eq-field-error']!==undefined)[0],row=nodes(tree,n=>n.props['data-eq-band-fields']!==undefined)[0],q=nodes(tree,n=>typeof n.type==='function'&&n.props['aria-label']===i18n.t('equalizer.q'))[0];
  assert.ok(error);assert.equal(control.props['aria-describedby'],error.props.id);assert.equal(error.props.role,'alert');assert.match(error.props.children,/-15.*15/);assert.equal(error.props.children,language==='en'?'Allowed gain: -15 to +15 dB.':'Ganho permitido: -15 a +15 dB.');assert.ok(q);
  assert.equal(row.props.children.length,4,'feedback renders outside the flex row, leaving its controls in the same geometry');
  control.props.onValidityChange(false);assert.equal(nodes(h.render(),n=>n.props['data-eq-field-error']!==undefined).length,0);
 }
 h.unmount();await i18n.changeLanguage('en');
});
test('load and draft edits do not activate; explicit Apply sends one complete draft',async()=>{const calls=[];const data=catalog();const h=host({get:async()=>{calls.push('get');return data},apply:async(...v)=>{calls.push(['apply',...v]);return{runtime:{...off,revision:2,phase:'ready',applied:v[1],verified:true},audioRoute:null}}});h.render();await flush();let tree=h.render();assert.deepEqual(calls,['get']);nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.addBand')))[0].props.onClick();tree=h.render();nodes(tree,n=>n.props.min===-15&&n.props.max===15&&n.props.onValueChange)[0].props.onValueChange(3);tree=h.render();assert.deepEqual(calls,['get']);by(tree,i18n.t('equalizer.apply')).props.onClick();await flush();assert.deepEqual(calls.map(v=>Array.isArray(v)?v[0]:v),['get','apply','get']);assert.equal(calls[1][1],true);assert.equal(calls[1][2].bands[0].gain_db,3);h.render({runtime:off});assert.equal(h.states[6].revision,2);h.unmount();});
test('Direct disables activation; ten accessible bands and translated UI in both languages',async()=>{const calls=[];const h=host({get:async()=>catalog(),apply:async()=>calls.push('apply')},{shared:false});h.render();await flush();for(const lang of ['en','pt-BR']){await i18n.changeLanguage(lang);const tree=h.render();assert.equal(by(tree,i18n.t('equalizer.apply')).props.disabled,true);assert.equal(nodes(tree,n=>n.type==='input'&&n.props.type==='checkbox'&&n.props['aria-label']!==i18n.t('equalizer.showGlobalGain')).length,0);assert.ok(nodes(tree,n=>n.type==='select'&&n.props['aria-label']===i18n.t('equalizer.selectBand')).length);const html=renderToStaticMarkup(tree);assert.doesNotMatch(html,/equalizer\.[a-z]/);assert.match(html,/aria-label/);}assert.deepEqual(calls,[]);h.unmount();await i18n.changeLanguage('en');});
test('unmount ignores late catalog; corrupt store never defaults into writable catalog',async()=>{let done;const h=host({get:()=>new Promise(r=>done=r)});h.render();h.unmount();done(catalog());await flush();assert.equal(h.states[0],null);const broken=host({get:async()=>{throw Error('broken')}});broken.render();await flush();assert.equal(broken.states[0],null);assert.equal(broken.states[5],'EQ_UNCONFIRMED');assert.equal(nodes(broken.render(),n=>n.type==='select').length,0);broken.unmount();});
test('IPC service uses explicit payloads without reads or transport commands',async()=>{const calls=[];const {equalizerService:s}=compile('services/equalizer.ts',{'@tauri-apps/api/core':{invoke:async(...args)=>calls.push(args)}});await s.apply(true,math.flatPreset());await s.edit('rev',{action:'select',reference:{kind:'built_in',id:'flat'}});assert.deepEqual(calls.map(c=>c[0]),['apply_equalizer','edit_equalizer']);assert.equal(calls[1][1].expectedRevision,'rev');});

if(process.env.SONANTE_EQ_PREVIEW_DIR)test('render private fixture preview',async()=>{const dir=process.env.SONANTE_EQ_PREVIEW_DIR;assert.ok(dir.endsWith('/outputs/sonante-eq-interaction'));const css=readdirSync(new URL('../dist/assets/',import.meta.url)).find(f=>f.endsWith('.css'));for(const lang of ['pt-BR','en']){await i18n.changeLanguage(lang);const h=host({get:async()=>catalog()});h.render();await flush();const html=renderToStaticMarkup(h.render());writeFileSync(`${dir}/preview-${lang}.html`,`<!doctype html><html lang="${lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><style>${readFileSync(new URL(`../dist/assets/${css}`,import.meta.url),'utf8')}body{margin:0;background:#131313;color:white;font-family:Arial,sans-serif}main{height:100vh}</style><body>${html}</body></html>`);h.unmount();}await i18n.changeLanguage('en');});

test('points have log coordinates, keyboard fine steps, ten reusable identities and finite gain protection',()=>{
 for(const x of [0,.1,.5,.9,1])assert.ok(Math.abs(math.frequencyX(math.xFrequency(x))-x)<1e-12);
 assert.equal(math.xFrequency(-10),20);assert.equal(math.xFrequency(10),20000);
 const p=math.flatPreset();let b=p.bands[0];assert.ok(math.movePoint(b,'ArrowRight',true).frequency_hz>b.frequency_hz);assert.ok(math.movePoint(b,'ArrowRight',true).frequency_hz<math.movePoint(b,'ArrowRight').frequency_hz);assert.equal(math.movePoint({...b,gain_db:15},'ArrowUp').gain_db,15);
 let budget=math.protection(p);assert.ok(budget.effective_preamp_db<=-1);const flatGain=budget.effective_preamp_db;p.preamp_db=12;budget=math.protection(p);assert.equal(budget.effective_preamp_db,flatGain);assert.equal(budget.attenuation_db,12-flatGain);for(const b of p.bands){b.enabled=true;b.frequency_hz=997;b.gain_db=12;}assert.equal(math.protection(p),null);
});
test('actual editor adds/removes/reuses slots without IPC and discards back to applied settings',async()=>{
 const calls=[];const h=host({get:async()=>catalog(),apply:async()=>calls.push('unexpected')});h.render();await flush();const add=tree=>nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.addBand')))[0];
 const originalIds=h.states[1].bands.map(b=>b.id);for(let i=0;i<10;i++){let tree=h.render();assert.equal(add(tree).props.disabled,false);add(tree).props.onClick()};let tree=h.render();assert.equal(add(tree).props.disabled,true);assert.equal(h.states[1].bands.filter(b=>b.enabled).length,10);assert.deepEqual(h.states[1].bands.map(b=>b.id),originalIds);assert.deepEqual(calls,[]);
 const remove=nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.removeBand')))[0];remove.props.onClick();assert.equal(h.states[1].bands.filter(b=>b.enabled).length,9);add(h.render()).props.onClick();assert.deepEqual(h.states[1].bands.map(b=>b.id),originalIds);
 const applied=math.flatPreset();applied.id='personal_fixture';applied.bands[0].enabled=true;applied.bands[0].gain_db=-2;tree=h.render({runtime:{...off,revision:5,phase:'active',applied,verified:true,in_path:true}});by(tree,i18n.t('equalizer.discard')).props.onClick();assert.ok(math.sameSettings(h.states[1],applied));assert.equal(h.states[1].id,originalIds.length?"flat":"");assert.deepEqual(calls,[]);h.unmount();
});
test('graph uses actual SVG points, keyboard, drag/cancel and no wheel interception',()=>{
 const ref={current:null};const changes=[],selection=[];const {EqGraph}=compile('components/EqGraph.tsx',{'react':{useRef:()=>ref,useMemo:f=>f(),useState:v=>[v,()=>{}],useEffect:()=>{}},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../utils/equalizer':math});const preset=math.flatPreset();preset.bands[0].enabled=true;
 const props={preset,budget:math.protection(preset),selected:null,onSelect:id=>selection.push(id),onChange:(...v)=>changes.push(v),onAdd:(...v)=>changes.push(['add',...v])};let tree=EqGraph(props);const svg=nodes(tree,n=>n.type==='svg')[0],circle=nodes(tree,n=>n.type==='circle')[0];assert.equal(svg.props.onWheel,undefined);assert.equal(circle.props.role,'button');assert.equal(circle.props.tabIndex,0);circle.props.onKeyDown({key:'ArrowUp',shiftKey:false,preventDefault(){}});assert.equal(changes.at(-1)[1].gain_db,.5);
 circle.props.onPointerDown({button:0,pointerId:1,preventDefault(){},currentTarget:{setPointerCapture(){},focus(){}}});svg.props.onPointerMove({clientX:2000,clientY:-1000,currentTarget:{getBoundingClientRect:()=>({left:0,top:0,width:1000,height:350})}});assert.equal(changes.at(-1)[1].frequency_hz,20000);assert.equal(changes.at(-1)[1].gain_db,15);svg.props.onPointerCancel();assert.deepEqual(changes.at(-1)[1],preset.bands[0]);circle.props.onKeyDown({key:'Delete',preventDefault(){},currentTarget:{ownerSVGElement:{focus(){}}}});assert.deepEqual(changes.at(-1)[1],{enabled:false});assert.equal(selection.at(-1),null);
});
test('the Audio preferences show only the EQ switch while the independent page reuses the existing editor',()=>{const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8'),settings=readFileSync(new URL('../src/components/SettingsModal.tsx',import.meta.url),'utf8');assert.match(app,/label=\{t\("equalizer\.title"\)\}/);assert.doesNotMatch(app,/label=\{t\("settings\.pages\.audio"\)\}/);assert.match(app,/equalizerActive[\s\S]*?<EqualizerView/);assert.match(settings,/<EqualizerToggle/);assert.doesNotMatch(settings,/<EqualizerView/);});
test('curves and gain budget match the Rust coefficients exported for the real runtime proof',()=>{
 const cases=JSON.parse(readFileSync(new URL('fixtures/equalizer-collection.json',import.meta.url),'utf8'));
 for(const c of cases){const p=c.preset;const report=math.protection(p);assert.ok(report,p.id);for(const key of ['effective_preamp_db','filter_bound_db','attenuation_db'])assert.ok(Math.abs(report[key]-c.protection[key])<1e-7,`${p.id} ${key}`);for(let i=0;i<10;i++){const b=math.coefficients(p.bands[i],48000),got=[b[0]/b[3],b[1]/b[3],b[2]/b[3],b[4]/b[3],b[5]/b[3]];for(let k=0;k<5;k++)assert.ok(Math.abs(got[k]-c.coefficients[i][k])<1e-12,`${p.id} band ${i}`);}}
});
test('bypass level return needs explicit profile-bound confirmation, not the first click',async()=>{
 const p=math.flatPreset(),calls=[];const active={...off,revision:5,phase:'active',applied:p,verified:true,in_path:true,protection:math.protection(p)};const h=host({get:async()=>catalog(),apply:async(...args)=>{calls.push(args);return{...off,revision:6}}},{runtime:active});h.render();await flush();let tree=h.render();nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.bypass')))[0].props.onClick();tree=h.render();assert.equal(calls.length,0);assert.equal(nodes(tree,n=>n.type==='dialog').length,1);by(tree,i18n.t('equalizer.confirmBypass')).props.onClick();await flush();assert.equal(calls.length,1);assert.equal(calls[0][0],false);assert.deepEqual(calls[0][2],p);h.unmount();
});
test('graph geometry accounts for letterboxing at narrow or tall aspect ratios',()=>{
 const p=math.graphPoint(28.8,23.8,{left:0,top:0,width:600,height:224});assert.ok(Math.abs(p.x-48)<1e-9);assert.ok(Math.abs(p.y-28)<1e-9);assert.equal(math.graphPoint(0,0,{left:0,top:0,width:0,height:0}),null);
 assert.equal(math.validDraft({...math.flatPreset(),bands:null}),false);assert.equal(math.validDraft({...math.flatPreset(),id:undefined}),false);
});
test('actual App shortcut caller cannot change volume or transport from EQ points/cards/dialog',()=>{
 const source=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let handler;const visit=n=>{if(ts.isVariableDeclaration(n)&&n.name.getText(ast)==='handleKeyDown')handler=n.initializer.getText(ast);ts.forEachChild(n,visit)};visit(ast);assert.ok(handler);const code=ts.transpileModule(`const handler=${handler};`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;const invoke=new Function(`${code};return handler;`)();
 for(const key of ['ArrowUp','ArrowDown','ArrowLeft','ArrowRight',' ','m','Escape']){let checked=0;assert.doesNotThrow(()=>invoke({key,code:key,target:{closest:selector=>{assert.equal(selector,'[data-equalizer]');checked++;return{}}}}));assert.equal(checked,1);}
});

test('reference route is labeled honestly; unsaved draft survives and global gain is only a graph view',async()=>{
 const p=math.flatPreset();p.name='Source';p.preamp_db=-4;const ref={...off,revision:8,phase:'ready',verified:true,in_path:true,applied:null,comparison:{token:'owned',side:'reference',source:p,effective_gain_db:-17.04,interrupted:false},protection:math.protection(p)};
 const calls=[];const h=host({get:async()=>catalog(),apply:async(...v)=>calls.push(v),compare:async(...v)=>calls.push(v)},{runtime:ref});h.render();await flush();let tree=h.render();const html=renderToStaticMarkup(tree);assert.match(html,/Reference A/);assert.match(html,/Comparison preset/);assert.doesNotMatch(html,/Routed preset: Source/);
 const graph=nodes(tree,n=>typeof n.type==='function'&&n.props.onAdd)[0];assert.equal(graph.props.showGain,false);graph.props.onToggleGain();tree=h.render();assert.equal(nodes(tree,n=>typeof n.type==='function'&&n.props.onAdd)[0].props.showGain,true);assert.deepEqual(calls,[]);h.unmount();
});


test('native PulseAudio explains and blocks both activation controls, but presets remain editable',async()=>{
 const calls=[];const data={...catalog(),pulse_audio:true};
 const h=host({get:async()=>data,apply:async()=>calls.push('apply')});
 h.render();await flush();
 for(const lang of ['en','pt-BR']){
  await i18n.changeLanguage(lang);const tree=h.render();const html=renderToStaticMarkup(tree);
  assert.equal(i18n.t('equalizer.pulseAudioTitle'),lang==='en'?'Equalizer unavailable':'Equalizador indisponível');
  assert.equal(i18n.t('equalizer.pulseAudioNotice'),lang==='en'
   ?"Sonante's equalizer currently requires PipeWire. Your system is using native PulseAudio. Normal playback in Shared mode remains available, but EQ cannot be activated in this environment."
   :'O equalizador do Sonante atualmente requer PipeWire. Seu sistema está utilizando PulseAudio nativo. A reprodução normal no modo Compartilhado continua disponível, mas o EQ não pode ser ativado neste ambiente.');
  assert.ok(html.includes(i18n.t('equalizer.pulseAudioTitle')));
  assert.equal(nodes(tree,n=>n.props.id==='eq-pulseaudio-notice')[0].props.children[1].props.children,i18n.t('equalizer.pulseAudioNotice'));
  assert.ok(html.includes(i18n.t('equalizer.pulseAudioState')));
  assert.equal(nodes(tree,n=>n.props.id==='eq-pulseaudio-notice').length,1);
  const enable=nodes(tree,n=>n.type==='button'&&n.props.children?.includes?.(i18n.t('equalizer.enable')))[0];
  const apply=by(tree,i18n.t('equalizer.apply'));
  for(const button of [enable,apply]){
   assert.equal(button.props.disabled,true);assert.equal(button.props['aria-describedby'],'eq-pulseaudio-notice');
   button.props.onClick();await flush();
  }
  const library=nodes(tree,n=>n.type?.name==='EqLibrary')[0];assert.ok(library);assert.equal(library.props.disabled,false);
  assert.equal(nodes(tree,n=>n.props.role==='alert').length,0);assert.deepEqual(calls,[]);
 }
 h.unmount();await i18n.changeLanguage('en');
});

test('a server changed after loading is refused by backend and becomes an explicit UI block',async()=>{
 let data={...catalog(),pulse_audio:false};const calls=[];
 const h=host({get:async()=>data,apply:async()=>{calls.push('apply');throw 'EQ_PULSEAUDIO_UNSUPPORTED'}});
 h.render();await flush();by(h.render(),i18n.t('equalizer.apply')).props.onClick();await flush();
 let tree=h.render();assert.equal(calls.length,1);assert.equal(by(tree,i18n.t('equalizer.apply')).props.disabled,true);
 assert.equal(nodes(tree,n=>n.props.id==='eq-pulseaudio-notice').length,1);
 assert.equal(nodes(tree,n=>n.props.role==='alert').length,0);
 // An explicit availability check removes the block after a compatible server is observed.
 data={...catalog(),pulse_audio:false};by(tree,i18n.t('equalizer.recheckAvailability')).props.onClick();await flush();tree=h.render();
 assert.equal(by(tree,i18n.t('equalizer.apply')).props.disabled,false);
 assert.equal(nodes(tree,n=>n.props.id==='eq-pulseaudio-notice').length,0);h.unmount();
});

test('availability recheck in the editor preserves draft changes and never activates EQ',async()=>{
 let reads=0,applyCalls=0;const service={async get(){reads++;return{...catalog(),pulse_audio:reads===1}},async apply(){applyCalls++}};
 const h=host(service);h.render();await flush();let tree=h.render();
 assert.ok(nodes(tree,n=>n.props.id==='eq-pulseaudio-notice').length);
 nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.addBand')))[0].props.onClick();tree=h.render();
 assert.ok(h.states[1].bands.some(band=>band.enabled),'the editor has a local unsaved draft before rechecking');
 by(tree,i18n.t('equalizer.recheckAvailability')).props.onClick();await flush();tree=h.render();
 assert.equal(reads,2);assert.equal(nodes(tree,n=>n.props.id==='eq-pulseaudio-notice').length,0);
 assert.ok(h.states[1].bands.some(band=>band.enabled),'availability refresh must not replace the draft');
 assert.equal(by(tree,i18n.t('equalizer.apply')).props.disabled,false);assert.equal(applyCalls,0);
 h.unmount();
});

test('PulseAudio activation restriction preserves routed bypass and unrelated preset errors',async()=>{
 const calls=[];const p=math.flatPreset();
 const active={...off,revision:5,phase:'active',applied:p,verified:true,in_path:true};
 const h=host({get:async()=>({...catalog(),pulse_audio:true}),apply:async(...v)=>{calls.push(v);return{...off,revision:6}},edit:async()=>{throw Error('fixture store refusal')}},{runtime:active});
 h.render();await flush();let tree=h.render();
 const bypass=nodes(tree,n=>n.type==='button'&&n.props.children?.includes?.(i18n.t('equalizer.bypass')))[0];
 assert.equal(bypass.props.disabled,false);bypass.props.onClick();tree=h.render();
 by(tree,i18n.t('equalizer.confirmBypass')).props.onClick();await flush();assert.equal(calls.length,1);assert.equal(calls[0][0],false);
 tree=h.render();const library=nodes(tree,n=>n.type?.name==='EqLibrary')[0];
 await library.props.onEdit({action:'select',reference:{kind:'built_in',id:'flat'}});await flush();
 assert.equal(nodes(h.render(),n=>n.props.role==='alert').length,1);h.unmount();
});

test('Equalizer presents preset management in the single editor page and keeps drafts without applying DSP',async()=>{
 const applied=[],edits=[];const data=catalog();const userPreset={...math.flatPreset(),id:'user-1',name:'Existing'};const userReference={kind:'user',id:userPreset.id};data.config.active_preset=userReference;data.config.user_presets=[userPreset];const service={get:async()=>data,edit:async(_revision,action)=>{edits.push(action);return data},apply:async(...args)=>{applied.push(args);return{runtime:off}}};let returned=0;
 const h=host(service,{onReturn:()=>returned++});h.render();await flush();let tree=h.render();
 assert.equal(nodes(tree,n=>n.props?.role==='tab').length,0);
 const returnButton=nodes(tree,n=>n.type==='button'&&n.props.className?.includes('eq-return'))[0];assert.ok(returnButton);returnButton.props.onClick();assert.equal(returned,1);
 const editor=nodes(tree,n=>n.props?.['data-eq-area']==='editor')[0];assert.ok(editor);assert.equal(editor.props.hidden,undefined);
 const preamp=nodes(tree,n=>n.type==='input'&&n.props?.type==='range'&&n.props?.['aria-label']===i18n.t('equalizer.preamp'))[0];
 assert.ok(preamp);preamp.props.onChange({target:{value:'-4.5'}});tree=h.render();
 const library=nodes(tree,n=>n.type?.name==='EqLibrary')[0];assert.ok(library);
 assert.equal(library.props.draft.preamp_db,-4.5);
 assert.equal(library.props.draft, h.states[1]);
 const equalizerSource=readFileSync(new URL('../src/components/EqualizerView.tsx',import.meta.url),'utf8');
 assert.doesNotMatch(equalizerSource,/eq-library-tab|OpraCatalogBrowser|AutoEqView/);
 const order=[
  nodes(tree,n=>n.type==='header'&&n.props.className==='eq-header')[0],
  nodes(tree,n=>n.props?.className==='eq-route-actions')[0],
  nodes(tree,n=>n.props?.className==='eq-route-summary')[0],
  nodes(tree,n=>n.props?.className==='eq-toolbar')[0],
  nodes(tree,n=>n.type?.name==='EqComparison')[0],
  nodes(tree,n=>n.type?.name==='EqGraph')[0],
  nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(i18n.t('equalizer.addBand')))[0],
  nodes(tree,n=>n.props?.className?.includes('eq-preamp-section'))[0],
  nodes(tree,n=>n.type==='input'&&n.props?.['aria-label']===i18n.t('equalizer.name'))[0],
  nodes(tree,n=>n.type?.name==='EqLibrary')[0],
 ];
 assert.ok(order.every(Boolean),'all requested equalizer sections are rendered');
 const positions=order.map(node=>nodes(tree,n=>true).indexOf(node));assert.deepEqual(positions,[...positions].sort((a,b)=>a-b),'equalizer sections follow the requested top-to-bottom order');
 const librarySource=readFileSync(new URL('../src/components/EqLibrary.tsx',import.meta.url),'utf8');
 assert.match(librarySource,/equalizerService\.pickImport/);
 assert.match(librarySource,/equalizerService\.exportPreset/);
 assert.doesNotMatch(librarySource,/equalizer\.preset|equalizer\.saveAs/);
 assert.doesNotMatch(librarySource,/AutoEq|OPRA|pickAutoEq|import_autoeq/);
 const nameField=nodes(tree,n=>n.type==='input'&&n.props?.['aria-label']===i18n.t('equalizer.name'))[0];nameField.props.onChange({target:{value:'Copy'}});tree=h.render();
 by(tree,i18n.t('equalizer.saveAs')).props.onClick();await flush();
 tree=h.render();by(tree,i18n.t('equalizer.save')).props.onClick();await flush();
 tree=h.render();nodes(tree,n=>n.type==='input'&&n.props?.['aria-label']===i18n.t('equalizer.name'))[0].props.onChange({target:{value:'Renamed'}});tree=h.render();by(tree,i18n.t('equalizer.rename')).props.onClick();await flush();
 tree=h.render();nodes(tree,n=>n.type==='button'&&n.props?.['aria-label']===i18n.t('equalizer.delete'))[0].props.onClick();await flush();
 assert.deepEqual(edits.map(action=>action.action),['create','update','rename','delete']);
 assert.equal(applied.length,0);
 const restored=nodes(tree,n=>n.type==='input'&&n.props?.type==='range'&&n.props?.['aria-label']===i18n.t('equalizer.preamp'))[0];
 assert.equal(restored.props.value,0);assert.equal(applied.length,0);
 h.unmount();
});
