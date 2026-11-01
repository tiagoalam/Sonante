import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import {renderToPipeableStream} from 'react-dom/server';
import {Writable} from 'node:stream';
import {localDiagnosticErrors,logError} from '../src/utils/frontendDiagnostics.ts';
const read=file=>readFileSync(new URL('../src/'+file,import.meta.url),'utf8');
function compile(file,deps={}){const exports={};new Function('require','exports',ts.transpileModule(read(file),{fileName:file,compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{if(typeof deps==='function')return deps(name);assert.ok(name in deps,name);return deps[name]},exports);return exports;}
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve}};
function nodes(tree,predicate,result=[]){if(!tree||typeof tree!=='object')return result;if(Array.isArray(tree)){tree.forEach(n=>nodes(n,predicate,result));return result;}if(predicate(tree))result.push(tree);nodes(tree.props?.children,predicate,result);return result;}
function text(tree){if(tree==null||typeof tree==='boolean')return '';if(typeof tree==='string'||typeof tree==='number')return String(tree);if(Array.isArray(tree))return tree.map(text).join(' ');return text(tree.props?.children);}
test('secondary modules are not imported when the application imports its view wrappers; selected view receives unchanged props',async()=>{
 const loads=[],props=[];const {RadioView}=compile('components/deferredViews.tsx',name=>{
  if(name==='react')return{...React,default:React};if(name==='react/jsx-runtime')return jsx;if(name==='react-i18next')return{useTranslation:()=>({t:key=>key})};if(name==='../utils/frontendDiagnostics')return{logError(){}};
  loads.push(name);const component=p=>{props.push(p);return React.createElement('p',null,'Selected view');};return{RadioView:component};
 });assert.deepEqual(loads,[]);
 const status={current_media:null};const html=await new Promise((resolve,reject)=>{let output='';const target=new Writable({write(chunk,encoding,done){output+=chunk.toString();done();},final(done){resolve(output);done();}});const rendering=renderToPipeableStream(React.createElement(RadioView,{active:true,available:true,status}),{onAllReady(){rendering.pipe(target);},onError:reject});});assert.match(html,/Selected view/);assert.deepEqual(loads,['./RadioView']);assert.ok(props.length>=1);assert.equal(props[0].status,status);assert.equal(props[0].active,true);
});
test('view failure retries only that view wrapper and reports a sanitized operation without navigating or reloading the app',()=>{
 const slots=[];let cursor=0;const react={...React,default:React,useState(initial){const index=cursor++;if(!(index in slots))slots[index]=initial;return[slots[index],value=>slots[index]=value(slots[index])]},useMemo(fn){cursor++;return fn()}};const errors=[];
 const exports=compile('components/deferredViews.tsx',name=>{if(name==='react')return react;if(name==='react/jsx-runtime')return jsx;if(name==='react-i18next')return{useTranslation:()=>({t:key=>key})};if(name==='../utils/frontendDiagnostics')return{logError:(...args)=>errors.push(args)};throw Error(name);});
 const render=()=>{cursor=0;return exports.RadioView({active:true,available:true,status:{}});};const first=render();const boundary=new first.type(first.props);boundary.state=first.type.getDerivedStateFromError(Error('fixture'));boundary.componentDidCatch('fixture');const button=nodes(boundary.render(),node=>node.type==='button')[0];assert.equal(text(button),'app.retryView');button.props.onClick();const second=render();assert.notEqual(second.key,first.key);assert.equal(errors[0][0],'app.view-load');
});
test('local diagnostic history is bounded, detached and never retains unknown messages, paths, URLs, credentials or error objects',()=>{
 const original=console.error;console.error=()=>{};try{for(let i=0;i<100;i++)logError('navidrome.catalog',`NAVIDROME_AUTH: http://private/track?password=SECRET /home/private (os error 2)`);const report=localDiagnosticErrors();assert.equal(report.length,32);const serialized=JSON.stringify(report);for(const secret of ['http:','private','SECRET','password','/home'])assert.ok(!serialized.includes(secret));assert.ok(serialized.includes('NAVIDROME_AUTH'));report[0].diagnostic.code='PRIVATE_MUTATION';assert.ok(!JSON.stringify(localDiagnosticErrors()).includes('PRIVATE_MUTATION'));report.length=0;assert.equal(localDiagnosticErrors().length,32);}finally{console.error=original;}
});
async function diagnosticFixture(options={}){
 let cursor=0,disposed=false;const slots=[],effects=[],calls=[],copied=[];
 const previous=Object.getOwnPropertyDescriptor(globalThis,'navigator');Object.defineProperty(globalThis,'navigator',{configurable:true,value:{clipboard:{writeText:async text=>{if(options.clipboardFailure)throw Error('unavailable');copied.push(text);}}}});
 const react={...React,default:React,useState(initial){const index=cursor++;if(!(index in slots))slots[index]=initial;return[slots[index],value=>{assert.equal(disposed,false);slots[index]=value;}]},useRef(initial){return slots[cursor++]??={current:initial}},useEffect(fn){const index=cursor++;if(!(index in slots))effects.push(()=>{slots[index]={cleanup:fn()};});}};
 const {LocalDiagnostics}=compile('components/LocalDiagnostics.tsx',{react,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:key=>key})},'@tauri-apps/api/core':{invoke:async command=>{calls.push(command);return options.invoke?options.invoke():{schema:1,version:'0.6.0',entries:[]};}},'../utils/frontendDiagnostics':{localDiagnosticErrors:()=>[]},'../utils/frontendStartup':{frontendStartup:{snapshot:()=>({renderer_entry:10})}}});
 const render=()=>{cursor=0;const tree=LocalDiagnostics();effects.splice(0).forEach(fn=>fn());return tree;};return{render,calls,copied,dispose(){slots.forEach(slot=>slot?.cleanup?.());disposed=true;if(previous)Object.defineProperty(globalThis,'navigator',previous);else delete globalThis.navigator;}};
}
test('Copy diagnostics does no work until clicked, reads only the in-memory diagnostic command and supports manual copy fallback',async()=>{
 for(const clipboardFailure of [false,true]){const f=await diagnosticFixture({clipboardFailure});try{let tree=f.render();assert.deepEqual(f.calls,[]);nodes(tree,n=>n.type==='button')[0].props.onClick();await flush();tree=f.render();assert.deepEqual(f.calls,['get_local_diagnostics']);if(clipboardFailure){const textarea=nodes(tree,n=>n.type==='textarea')[0];assert.equal(textarea.props.readOnly,true);assert.ok(textarea.props.value.includes('Sonante'));}else{assert.ok(text(tree).includes('diagnostic.copied'));assert.equal(f.copied.length,1);}}finally{f.dispose();}}
});
test('closing diagnostics during its native read neither copies to the clipboard nor writes to the unmounted component',async()=>{
 const pending=deferred();const f=await diagnosticFixture({invoke:()=>pending.promise});const tree=f.render();nodes(tree,n=>n.type==='button')[0].props.onClick();f.dispose();pending.resolve({schema:1,version:'0.6.0',entries:[]});await flush();assert.equal(f.copied.length,0);
});

test('extracted window-title effect preserves presentation and avoids IPC on elapsed-only polling or unchanged identity',async()=>{
 const calls=[],errors=[];let previous;
 const {useAppWindowTitle}=compile('utils/useAppWindowTitle.ts',{react:{useEffect(fn,deps){if(!previous||deps.some((v,i)=>!Object.is(v,previous[i]))){previous=deps;fn();}}},'../services/audio':{audioService:{setWindowTitle:async title=>{calls.push(title);}}},'./frontendDiagnostics':{logError:(...args)=>errors.push(args)}});
 const status={state:'play',title:'Track',artist:'Artist',elapsed:1};useAppWindowTitle(false,status);useAppWindowTitle(true,status);useAppWindowTitle(true,{...status,elapsed:2});useAppWindowTitle(true,{...status,artist:'',elapsed:3});useAppWindowTitle(true,{...status,state:'pause'});assert.deepEqual(calls,['Sonante','Sonante • Artist — Track','Sonante • Track','Sonante']);assert.deepEqual(errors,[]);
});

test('renderer startup measurements record only fixed first-stage numbers and remain passive across replay and copies',()=>{
 const {createFrontendStartup}=compile('utils/frontendStartup.ts');let time=10,calls=0;const startup=createFrontendStartup(()=>{calls++;return time;});startup.mark('renderer_entry');time=20;startup.mark('renderer_entry');startup.mark('shell_committed');startup.mark('private://server');time=NaN;startup.mark('config_committed');time=30;startup.mark('config_committed');assert.deepEqual(startup.snapshot(),{renderer_entry:10,shell_committed:20,config_committed:30});assert.equal(calls,4);const copy=startup.snapshot();copy.renderer_entry=999;assert.equal(startup.snapshot().renderer_entry,10);
});
