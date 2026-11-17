import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import * as math from '../src/utils/equalizer.ts';
import * as audioExperience from '../src/utils/audioExperience.ts';
import {equalizerErrorCode} from '../src/utils/equalizerError.ts';

const i18n=i18next.createInstance();
await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(language=>[language,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`,import.meta.url),'utf8'))}]))});
const flush=async()=>{for(let i=0;i<24;i++)await Promise.resolve()};
const nodes=tree=>Array.isArray(tree)?tree.flatMap(nodes):!tree||typeof tree!=='object'?[]:[tree,...nodes(tree.props?.children)];
const toggle=tree=>nodes(tree).find(node=>node.type==='input'&&node.props.role==='switch');
const find=(tree,predicate)=>nodes(tree).find(predicate);
function compile(dependencies){
 const source=readFileSync(new URL('../src/components/EqualizerToggle.tsx',import.meta.url),'utf8');
 const code=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
 const exports={};new Function('require','exports',code)(name=>{assert.ok(name in dependencies,name);return dependencies[name]},exports);return exports.EqualizerToggle;
}
const off={in_path:false,revision:0,phase:'off',applied:null,error:null,pcm_rate:48000,pcm_channels:2,verified:false};
const catalog=(runtime=off,config={enabled:false,active_preset:{kind:'built_in',id:'flat'},user_presets:[]},pulse=false)=>({config:{schema_version:1,...config},revision:'fresh',built_ins:[math.flatPreset()],runtime,shared:true,runtime_available:true,pulse_audio:pulse});
function host(service,{shared=true,available=true,runtime=off,audioRoute=null,disabled=false}={}){
 const slots=[],pendingEffects=[];let cursor=0;const cleanups=[];
 const hooks={...React,useState(initial){const index=cursor++;slots[index]??=typeof initial==='function'?initial():initial;return[slots[index],value=>{slots[index]=typeof value==='function'?value(slots[index]):value}]},useRef(initial){const index=cursor++;return slots[index]??={current:initial}},useEffect(effect,deps){const index=cursor++,previous=slots[index];if(!previous||deps.some((value,i)=>!Object.is(value,previous.deps[i])))pendingEffects.push(()=>{previous?.cleanup?.();slots[index]={deps,cleanup:effect()}})}};
 const EqualizerToggle=compile({'react':hooks,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'lucide-react':icons,'../services/equalizer':{equalizerService:service},'../utils/audioExperience':audioExperience,'../utils/equalizerError':{equalizerErrorCode},'../utils/equalizer':math});
 return{render(){cursor=0;const tree=EqualizerToggle({shared,available,runtime,audioRoute,disabled});for(const effect of pendingEffects.splice(0))effect();return tree},slots,dispose(){for(const slot of slots)slot?.cleanup?.()}};
}

test('the switch persists desired state while checked state still comes from confirmed runtime',async()=>{
 const calls=[],data=catalog();const active={...off,revision:1,phase:'active',applied:math.flatPreset(),verified:true,in_path:true};
 const service={async get(){calls.push(['get']);return data},async apply(...args){calls.push(['apply',...args]);data.config.enabled=args[0];return active}};
 const h=host(service);let tree=h.render();assert.equal(toggle(tree).props.disabled,true);await flush();tree=h.render();assert.equal(toggle(tree).props.checked,false);assert.equal(toggle(tree).props.disabled,false);
 toggle(tree).props.onChange({currentTarget:{checked:true}});tree=h.render();assert.equal(toggle(tree).props.checked,false,'operation does not look confirmed while IPC is pending');assert.equal(toggle(tree).props.disabled,true);
 await flush();tree=h.render();assert.equal(toggle(tree).props.checked,true);assert.deepEqual(calls.map(call=>call[0]),['get','get','apply','get']);assert.equal(calls.at(-2)[1],true);assert.equal(calls.at(-2)[2].id,'flat');assert.equal(calls.at(-2)[3],null);assert.equal(data.config.enabled,true);
 h.dispose();
});

test('exclusive mode disables activation and reports the short Shared-only explanation',async()=>{
 const calls=[],service={async get(){return catalog()},async apply(...args){calls.push(args)}};const h=host(service,{shared:false});await flush();const tree=h.render();
 assert.equal(toggle(tree).props.disabled,true);assert.equal(toggle(tree).props.checked,false);assert.match(tree.props.children[1].props.children,/Available in Shared mode/);
 toggle(tree).props.onChange({currentTarget:{checked:true}});await flush();assert.equal(calls.length,0);h.dispose();
});

test('turning off asks for the existing bypass confirmation and a failure leaves the confirmed switch on',async()=>{
 const preset=math.flatPreset(),active={...off,revision:3,phase:'active',applied:preset,verified:true,in_path:true};const calls=[];
 const service={async get(){return catalog(active,{enabled:false,active_preset:{kind:'built_in',id:'flat'},user_presets:[]})},async apply(...args){calls.push(args);throw 'EQ_RUNTIME_UNCONFIRMED'}};
 const h=host(service,{runtime:active});h.render();await flush();let tree=h.render();assert.equal(toggle(tree).props.checked,true);
 toggle(tree).props.onChange({currentTarget:{checked:false}});tree=h.render();assert.equal(calls.length,0);const dialog=find(tree,node=>node.type==='dialog');assert.ok(dialog);assert.equal(toggle(tree).props.checked,true);
 find(tree,node=>node.type==='button'&&node.props.children==='Bypass at original level').props.onClick();await flush();tree=h.render();
 assert.equal(calls.length,1);assert.equal(calls[0][0],false);assert.deepEqual(calls[0][2],preset);assert.equal(toggle(tree).props.checked,true);assert.ok(find(tree,node=>node.props.role==='alert'));
 h.dispose();
});

test('a confirmed explicit disable is saved without changing the active preset',async()=>{
 const preset=math.flatPreset(),active={...off,revision:6,phase:'active',applied:preset,verified:true,in_path:true};const data=catalog(active,{enabled:true,active_preset:{kind:'built_in',id:'flat'},user_presets:[]});const calls=[];
 const service={async get(){calls.push('get');return data},async apply(...args){calls.push(['apply',...args]);data.config.enabled=args[0];return{...off,revision:7}}};
 const h=host(service,{runtime:active});h.render();await flush();let tree=h.render();toggle(tree).props.onChange({currentTarget:{checked:false}});tree=h.render();
 find(tree,node=>node.type==='button'&&node.props.children==='Bypass at original level').props.onClick();await flush();tree=h.render();
 assert.equal(data.config.enabled,false);assert.deepEqual(data.config.active_preset,{kind:'built_in',id:'flat'});assert.equal(toggle(tree).props.checked,false);assert.deepEqual(calls.map(call=>typeof call==='string'?call:call[0]),['get','get','apply','get']);h.dispose();
});

test('a failed enable does not claim EQ is active and offers diagnostic details only on demand',async()=>{
 const service={async get(){return catalog()},async apply(){throw 'EQ_RUNTIME_UNCONFIRMED'}};const h=host(service);h.render();await flush();let tree=h.render();toggle(tree).props.onChange({currentTarget:{checked:true}});await flush();tree=h.render();
 assert.equal(toggle(tree).props.checked,false);const details=find(tree,node=>node.type==='details');assert.ok(details);assert.equal(details.props.open,undefined);assert.match(details.props.children[0].props.children,/Details/);h.dispose();
});

test('a saved request without runtime confirmation is described as pending, not active',async()=>{
 const service={async get(){return catalog(off,{enabled:true})},async apply(){throw new Error('should not apply')}};const h=host(service);h.render();await flush();const tree=h.render();
 assert.equal(toggle(tree).props.checked,false);assert.match(JSON.stringify(tree),/EQ is saved to turn on when Shared output is ready/);h.dispose();
});

test('preset updates keep the confirmed EQ switch on while the runtime applies filters',async()=>{
 const applied=math.flatPreset(),updating={...off,revision:3,phase:'updating',applied,verified:true,in_path:true};
 const service={async get(){return catalog(updating,{enabled:true})},async apply(){throw new Error('not invoked')}};const h=host(service,{runtime:off});h.render();await flush();const tree=h.render();
 assert.equal(toggle(tree).props.checked,true);assert.equal(toggle(tree).props.disabled,true);assert.match(JSON.stringify(tree),/Applying preset/);h.dispose();
});

test('a failed startup restoration remains visibly unconfirmed',async()=>{
 const failed={...off,revision:4,phase:'error',error:'runtime_failed'};const service={async get(){return catalog(failed,{enabled:true})},async apply(){throw new Error('not invoked')}};const h=host(service,{runtime:failed});h.render();await flush();const tree=h.render();
 assert.equal(toggle(tree).props.checked,false);assert.match(nodes(tree).find(node=>node.props?.role==='alert')?.props.children,/EQ is still requested/);h.dispose();
});
