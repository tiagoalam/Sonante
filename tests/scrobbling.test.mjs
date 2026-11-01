import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
const read = file => readFileSync(new URL('../'+file,import.meta.url),'utf8');
const flush = () => new Promise(resolve=>setImmediate(resolve));
const deferred = () => {let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
function compile(file,deps){const out={};new Function('require','exports',ts.transpileModule(read('src/'+file),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},out);return out;}
function nodes(tree,predicate,result=[]){if(!tree||typeof tree!=='object')return result;if(Array.isArray(tree)){tree.forEach(child=>nodes(child,predicate,result));return result;}if(predicate(tree))result.push(tree);nodes(tree.props?.children,predicate,result);return result;}
function text(tree){if(tree===null||tree===undefined||typeof tree==='boolean')return '';if(typeof tree==='string'||typeof tree==='number')return String(tree);if(Array.isArray(tree))return tree.map(text).join(' ');return text(tree.props?.children);}
const service = compile('services/scrobbling.ts',{'@tauri-apps/api/core':{invoke(){}},'@tauri-apps/api/event':{listen(){}}});
const status=(revision=1,extra={})=>({revision,ready:true,storage_error:false,lastfm_authorizing:false,lastfm:{connected:false,enabled:false,username:null,queued:0,last_sent:null,error:null},listenbrainz:{connected:false,enabled:false,username:null,queued:0,last_sent:null,error:null},...extra});
function fixture(options={}){
 let cursor=0,disposed=false,event,unsubscribed=0;const slots=[],effects=[],actions=[],opened=[];let backend=status();
 const react={...React,default:React,useId:()=> 'scrobble-fixture',useState(initial){const i=cursor++;if(!(i in slots))slots[i]=typeof initial==='function'?initial():initial;return[slots[i],v=>{assert.equal(disposed,false,'no state update after unmount');slots[i]=typeof v==='function'?v(slots[i]):v}]},useRef(initial){return slots[cursor++]??={current:initial}},useEffect(fn,deps){const i=cursor++,old=slots[i];if(!old||deps.some((v,n)=>!Object.is(v,old.deps[n])))effects.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()}})}};
 const native={subscribe:async fn=>{event=fn;return()=>{unsubscribed++}},status:()=>options.read?options.read():Promise.resolve(backend),action:async action=>{actions.push(action);if(options.action)return options.action(action);if(action.kind==='listenbrainz_connect')backend=status(2,{listenbrainz:{...status().listenbrainz,connected:true,enabled:true,username:'fixture'}});return{status:backend,authorization_url:null}}};
 const {ScrobblingSetting}=compile('components/ScrobblingSetting.tsx',{react,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:(key,values)=>values?.user?`${key}:${values.user}`:key,i18n:{language:'en'}})},'@tauri-apps/plugin-opener':{openUrl:async url=>{opened.push(url);if(options.open)return options.open(url)}},'lucide-react':{ExternalLink:()=>null,LoaderCircle:()=>null,Music2:()=>null,RefreshCw:()=>null},'../services/scrobbling':{scrobblingService:native,scrobblingError:service.scrobblingError}});
 const render=()=>{cursor=0;const tree=ScrobblingSetting();effects.splice(0).forEach(fn=>fn());return tree};
 const button=(tree,key)=>{const found=nodes(tree,n=>n.type==='button'&&text(n).includes(key));assert.equal(found.length,1,key);return found[0]};
 return{render,button,actions,opened,event:value=>event(value),input:(tree,suffix,value)=>{const n=nodes(tree,n=>n.type==='input'&&n.props.id?.endsWith(suffix));assert.equal(n.length,1);n[0].props.onChange({target:{value}})},dispose(){slots.forEach(slot=>slot?.cleanup?.());disposed=true},get unsubscribed(){return unsubscribed}};
}
test('opt-in controls do not submit anything on mount and tokens are masked',async()=>{
 const f=fixture();try{let tree=f.render();await flush();tree=f.render();assert.deepEqual(f.actions,[]);assert.equal(f.button(tree,'scrobbling.connect').props.disabled,true);assert.equal(f.button(tree,'scrobbling.authorize').props.disabled,true);for(const input of nodes(tree,n=>n.type==='input'))assert.equal(input.props.type,'password');assert.equal(f.opened.length,0);}finally{f.dispose()}assert.equal(f.unsubscribed,1);
});
test('ListenBrainz connects explicitly, clears secret fields, then toggles and disconnects independently',async()=>{
 const f=fixture();try{let tree=f.render();await flush();tree=f.render();f.input(tree,'-token','SYNTHETIC-TOKEN');tree=f.render();assert.equal(f.button(tree,'scrobbling.connect').props.disabled,false);await f.button(tree,'scrobbling.connect').props.onClick();await flush();tree=f.render();assert.deepEqual(f.actions[0],{kind:'listenbrainz_connect',token:'SYNTHETIC-TOKEN'});assert.equal(nodes(tree,n=>n.type==='input'&&n.props.id?.endsWith('-token'))[0].props.value,'');assert.ok(text(tree).includes('scrobbling.connected:fixture'));const toggle=nodes(tree,n=>n.type==='input'&&n.props.type==='checkbox')[0];toggle.props.onChange({target:{checked:false}});await flush();assert.deepEqual(f.actions[1],{kind:'set_enabled',provider:'listenbrainz',enabled:false});tree=f.render();f.button(tree,'scrobbling.disconnect').props.onClick();await flush();assert.deepEqual(f.actions[2],{kind:'disconnect',provider:'listenbrainz'});assert.equal(f.opened.length,0);}finally{f.dispose()}
});
test('Last.fm browser authorization requires explicit confirmation and never accepts account passwords',async()=>{
 let calls=0;const f=fixture({action:async action=>{calls++;return calls===1?{status:status(2,{lastfm_authorizing:true}),authorization_url:'https://www.last.fm/api/auth/?api_key='+ 'a'.repeat(32)+'&token='+'b'.repeat(32)}:{status:status(3,{lastfm:{...status().lastfm,connected:true,enabled:true,username:'fixture'}}),authorization_url:null}}});
 try{let tree=f.render();await flush();tree=f.render();f.input(tree,'-key','a'.repeat(32));f.input(tree,'-secret','c'.repeat(32));tree=f.render();f.button(tree,'scrobbling.authorize').props.onClick();await flush();tree=f.render();assert.equal(f.actions.length,1);assert.equal(f.actions[0].kind,'lastfm_begin');assert.equal(f.opened.length,1);assert.ok(text(tree).includes('scrobbling.authorizeHelp'));f.button(tree,'scrobbling.authorized').props.onClick();await flush();tree=f.render();assert.deepEqual(f.actions[1],{kind:'lastfm_finish'});assert.ok(text(tree).includes('scrobbling.connected:fixture'));assert.equal(nodes(tree,n=>n.type==='input'&&n.props.id?.endsWith('-secret')).length,0);}finally{f.dispose()}
});
test('a late initial read cannot erase a newer native connection event',async()=>{
 const read=deferred();const f=fixture({read:()=>read.promise});try{f.render();await flush();f.event(status(5,{listenbrainz:{...status().listenbrainz,connected:true,enabled:true,username:'newer'}}));read.resolve(status(1));await flush();assert.ok(text(f.render()).includes('scrobbling.connected:newer'));}finally{f.dispose()}
});
test('unmount fences pending connection UI and prevents opening an abandoned auth browser',async()=>{
 const action=deferred();const f=fixture({action:()=>action.promise});let tree=f.render();await flush();tree=f.render();f.input(tree,'-key','a'.repeat(32));f.input(tree,'-secret','c'.repeat(32));tree=f.render();f.button(tree,'scrobbling.authorize').props.onClick();f.dispose();action.resolve({status:status(2,{lastfm_authorizing:true}),authorization_url:'https://www.last.fm/api/auth/?api_key=x&token=y'});await flush();assert.equal(f.opened.length,0);assert.equal(f.unsubscribed,1);
});
test('raw service errors and credential values never reach user-facing diagnostics',async()=>{
 for(const raw of ['https://private.invalid/?token=SECRET',new Error('SECRET'),{token:'SECRET'}])assert.equal(service.scrobblingError(raw),'unavailable');assert.equal(service.scrobblingError('SCROBBLING_NOT_AUTHORIZED'),'not_authorized');
 const f=fixture({action:async()=>{throw 'https://private.invalid/?token=SECRET'}});try{let tree=f.render();await flush();tree=f.render();f.input(tree,'-token','SYNTHETIC-TOKEN');tree=f.render();f.button(tree,'scrobbling.connect').props.onClick();await flush();tree=f.render();assert.ok(text(tree).includes('scrobbling.errors.unavailable'));assert.ok(!text(tree).includes('SECRET'));}finally{f.dispose()}
});

test('an unexpected connection reply never promises automatic submission retry or opens authorization',async()=>{
 const f=fixture({action:async()=>{throw 'SCROBBLING_PROTOCOL'}});try{let tree=f.render();await flush();tree=f.render();f.input(tree,'-key','a'.repeat(32));f.input(tree,'-secret','c'.repeat(32));tree=f.render();f.button(tree,'scrobbling.authorize').props.onClick();await flush();tree=f.render();assert.ok(text(tree).includes('scrobbling.errors.connection_protocol'));assert.ok(!text(tree).includes('scrobbling.errors.protocol'));assert.equal(f.opened.length,0);assert.equal(f.actions.length,1);}finally{f.dispose()}
});
