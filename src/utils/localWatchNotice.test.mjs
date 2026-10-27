import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
const source=readFileSync(new URL('../components/LocalWatchNotice.tsx',import.meta.url),'utf8');
const flush=async()=>{for(let i=0;i<20;i++)await Promise.resolve();};
function mount(lang='pt-BR',query=async()=>false,report=null){
 const strings=JSON.parse(readFileSync(new URL(`../locales/${lang}.json`,import.meta.url),'utf8'));
 const t=key=>key.split('.').reduce((v,k)=>v[k],strings);
 const listeners=new Map(),cleaned=[];let cursor=0,effect,cleanup,invocations=0,opened=0;const states=[];
 const deps={'../services/indexCoverage':{useIndexCoverage:()=>({revision:1,report})},'./LocalIndexCoverage':{IndexCoverageDetails:()=>{throw new Error('technical details must stay out of the toast')}},react:{useState:initial=>{const i=cursor++;if(!(i in states))states[i]=initial;return[states[i],v=>{states[i]=v}];},useEffect:fn=>{effect??=fn;}},'@tauri-apps/api/core':{invoke:async cmd=>{assert.equal(cmd,'get_local_watch_needs_check');invocations++;return query();}},'@tauri-apps/api/event':{listen:async(event,fn)=>{listeners.set(event,fn);return()=>{listeners.delete(event);cleaned.push(event);};}},'react-i18next':{useTranslation:()=>({t})},'lucide-react':icons,'react/jsx-runtime':jsx};
 const exports={};new Function('require','exports',ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name];},exports);
 const draw=()=>{cursor=0;return exports.LocalWatchNotice({onOpenSettings:()=>opened++});};
 draw();cleanup=effect();
 return {listeners,cleaned,draw,html:()=>renderToStaticMarkup(draw()),close:()=>cleanup(),get calls(){return invocations;},get opened(){return opened;}};
}
function find(node,predicate){if(!React.isValidElement(node))return null;if(predicate(node))return node;for(const child of React.Children.toArray(node.props.children)){const found=find(child,predicate);if(found)return found;}return null;}
test('real notice subscribes once, loads persisted issue, translates and opens Local preferences',async()=>{
 for(const lang of ['pt-BR','en']){const f=mount(lang,async()=>true);await flush();const html=f.html();assert.match(html,/role="alert"/);assert.ok(html.includes(lang==='pt-BR'?'monitoramento automático':'Automatic library monitoring'));
 const button=find(f.draw(),n=>n.type==='button'&&!n.props['aria-label']);button.props.onClick();assert.equal(f.opened,1);assert.equal(f.calls,1);for(let i=0;i<10;i++)f.draw();assert.equal(f.calls,1);f.close();assert.equal(f.cleaned.length,2);}
});
test('ready notification clears issue and stale startup query cannot restore it',async()=>{
 let resolve;const f=mount('pt-BR',()=>new Promise(r=>resolve=r));await flush();f.listeners.get('local-library-watch-needs-check')();assert.match(f.html(),/role="alert"/);f.listeners.get('local-library-watch-ready')();resolve(true);await flush();assert.equal(f.html(),'');f.close();
});
test('dismiss does not run repairs; cleanup ignores late queries and callbacks',async()=>{
 const f=mount('pt-BR',async()=>true);await flush();find(f.draw(),n=>n.type==='button'&&n.props['aria-label']).props.onClick();assert.equal(f.html(),'');const callback=f.listeners.get('local-library-watch-needs-check');f.close();callback();assert.equal(f.html(),'');
});
test('App mounts the real notice and routes its action to Local settings without library mutation',()=>{
 const app=readFileSync(new URL('../App.tsx',import.meta.url),'utf8');const ast=ts.createSourceFile('App.tsx',app,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let action;
 const visit=n=>{if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==='LocalWatchNotice')action=n.attributes.properties.find(a=>a.name?.getText(ast)==='onOpenSettings').initializer.expression.getText(ast);ts.forEachChild(n,visit);};visit(ast);assert.ok(action);
 const calls=[];new Function('setSettingsInitialPage','setShowSettings',`return (${action})`)(v=>calls.push(['page',v]),v=>calls.push(['show',v]))();assert.deepEqual(calls,[['page','local'],['show',true]]);
});

test('partial notice stays short with details only by explicit Preferences action; dismissal never repairs',async()=>{
 for(const lang of ['pt-BR','en']){
  const report={outcome:'partial',scope:'full',generation:'fixture',sources:[{source:'Fixture',audio_files:1033,cataloged_audio:1033,cue_groups:16,virtual_tracks:122,omitted:1,policy_excluded:322,audio_unconfirmed:0,cue_groups_unobserved:1,unaddressable:0}]};
  const f=mount(lang,async()=>false,report);await flush();const html=f.html();
  assert.match(html,/role="alert"/);assert.doesNotMatch(html,/<details|1033|322|CUE|scan|corrupt|rescan/i);
  assert.ok(html.includes(lang==='pt-BR'?'Ver detalhes nas Preferências':'View details in Preferences'));
  find(f.draw(),n=>n.type==='button'&&!n.props['aria-label']).props.onClick();assert.equal(f.opened,1);assert.equal(f.calls,1);
  find(f.draw(),n=>n.type==='button'&&n.props['aria-label']).props.onClick();assert.equal(f.html(),'');assert.equal(f.calls,1);f.close();
 }
});
