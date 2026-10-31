import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {appearancePalettes} from '../src/utils/appearance.ts';

const read=file=>readFileSync(new URL('../'+file,import.meta.url),'utf8');
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
const settle=async()=>{for(let n=0;n<20;n++)await Promise.resolve();};
const luminance=h=>[1,3,5].map(i=>parseInt(h.slice(i,i+2),16)/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4).reduce((n,v,i)=>n+v*[.2126,.7152,.0722][i],0);
const contrast=(a,b)=>{const[x,y]=[luminance(a),luminance(b)].sort((a,b)=>a-b);return(y+.05)/(x+.05);};
test('new theme previews match actual CSS and primary, secondary, muted and accent text retain contrast',()=>{
 const css=read('src/index.css');
 for(const theme of ['cyber','summer','zen','nocturne']){
  const block=css.split(`:root[data-theme="${theme}"] {`)[1].split('}')[0];
  const values=Object.fromEntries([...block.matchAll(/--theme-([\w-]+):\s*(#[\da-fA-F]{6})\s*;/g)].map(m=>[m[1],m[2]]));
  const palette=appearancePalettes[theme];
  assert.equal(values['surface-base'],palette.base);assert.equal(values['surface-panel'],palette.panel);assert.equal(values['surface-raised'],palette.card);assert.equal(values['fg-primary'],palette.text);assert.equal(values['fg-muted'],palette.muted);assert.equal(values['control-accent'],palette.gold);
  for(const fg of ['fg-primary','fg-secondary','fg-muted','fg-accent'])for(const bg of ['surface-base','surface-panel','surface-raised','surface-hover','surface-accent'])assert.ok(contrast(values[fg],values[bg])>=4.5,`${theme}: ${fg}/${bg}=${contrast(values[fg],values[bg])}`);
  assert.ok(contrast(values['control-fg'],values['control-accent'])>=4.5,theme+' control');
  for(const bg of ['surface-base','surface-panel','surface-raised','surface-hover'])assert.ok(contrast(values['border-accent'],values[bg])>=3,theme+' focus/'+bg);
 }
});

test('Ctrl+K opens the unified search and respects modal ownership without audio effects',()=>{
 const source=read('src/App.tsx');const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let handler;
 const visit=node=>{if(ts.isVariableDeclaration(node)&&node.name.getText(ast)==='handleKeyDown'&&node.initializer?.getText(ast).includes('sonante:focus-unified-search'))handler=node.initializer.getText(ast);ts.forEachChild(node,visit);};visit(ast);assert.ok(handler);
 const js=ts.transpileModule(`const action=(${handler});`,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 for(const guarded of [null,'settings','about','dialog','prevented','wizard','bootstrap']){
  const calls=[];const env={config:guarded==='bootstrap'?null:{first_run:guarded==='wizard'},showSettings:guarded==='settings',showAbout:guarded==='about',document:{querySelector:()=>guarded==='dialog'?{}:null},resetAllNavigation:()=>calls.push('reset'),mainSectionPreference:{navigate:section=>calls.push(section)},window:{dispatchEvent:()=>calls.push('focus')},Event,nowPlayingHosting:{getSnapshot:()=>({mode:'closed',busy:false})}};
  const action=new Function(...Object.keys(env),js+'return action;')(...Object.values(env));
  action({ctrlKey:true,key:'K',defaultPrevented:guarded==='prevented',preventDefault:()=>calls.push('prevent')});
  assert.deepEqual(calls,guarded?[]:['prevent','reset','search','focus']);
 }
});

test('UPnP preference UI requires an explicit Apply and cannot report success from rejection or late completion',async()=>{
 let cursor=0;const slots=[],effects=[],calls=[],pending=[];
 const hooks={...React,useId:()=> 'receiver',useState:initial=>{const n=cursor++;slots[n]??={value:initial};return[slots[n].value,value=>slots[n].value=value];},useRef:value=>{const n=cursor++;return slots[n]??={current:value};},useEffect:(fn)=>{const n=cursor++;if(!slots[n]){slots[n]={};effects.push(()=>slots[n].cleanup=fn());}}};
 const imports={react:hooks,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:key=>key})},'lucide-react':icons,'@tauri-apps/api/core':{invoke:(...args)=>{calls.push(args);return new Promise((resolve,reject)=>pending.push({resolve,reject}));}}};
 const code=ts.transpileModule(read('src/components/UpnpSetting.tsx'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;const exports={};new Function('require','exports',code)(id=>{assert.ok(id in imports,id);return imports[id];},exports);
 const paint=()=>{cursor=0;const tree=exports.UpnpSetting();effects.splice(0).forEach(fn=>fn());return tree;};
 let tree=paint();assert.deepEqual(calls.map(c=>c[0]),['get_upnp_receiver']);
 pending.shift().resolve({enabled:false,requested_enabled:true,name:'Sonante',port:null,error:'UPNP_NETWORK_UNAVAILABLE'});await settle();tree=paint();
 assert.equal(nodes(tree,n=>n.type==='input'&&n.props.type==='checkbox')[0].props.checked,true);assert.equal(nodes(tree,n=>n.type==='button')[0].props.disabled,false);
 nodes(tree,n=>n.type==='input'&&n.props.type==='checkbox')[0].props.onChange({target:{checked:false}});tree=paint();assert.equal(nodes(tree,n=>n.type==='button')[0].props.disabled,false);
 nodes(tree,n=>n.type==='input'&&n.props.type==='checkbox')[0].props.onChange({target:{checked:true}});tree=paint();assert.equal(calls.length,1);
 nodes(tree,n=>n.type==='button')[0].props.onClick();assert.equal(calls[1][0],'set_upnp_receiver');assert.deepEqual(calls[1][1],{enabled:true,name:'Sonante'});
 pending.shift().reject('private low-level failure');await settle();tree=paint();assert.equal(nodes(tree,n=>n.props?.role==='alert').length,1);assert.equal(nodes(tree,n=>n.props?.children==='upnp.active').length,0);
 nodes(tree,n=>n.type==='button')[0].props.onClick();slots.forEach(slot=>slot?.cleanup?.());pending.shift().resolve({enabled:true,name:'Sonante',port:123,error:null});await settle();
 tree=paint();assert.equal(nodes(tree,n=>n.props?.children==='upnp.active').length,0);
});


test('new palettes keep warning, error, success and information readable without recoloring artwork',()=>{
 const css=read('src/index.css');
 const shared=css.split(':root:is([data-theme="light"], [data-theme="summer"], [data-theme="zen"]) {')[1].split('}')[0];
 const values=block=>Object.fromEntries([...block.matchAll(/--theme-([\w-]+):\s*(#[\da-fA-F]{6})\s*;/g)].map(m=>[m[1],m[2]]));
 for(const theme of ['cyber','summer','zen','nocturne']){
  const block=css.split(`:root[data-theme="${theme}"] {`)[1].split('}')[0];
  const tokens={...(['summer','zen'].includes(theme)?values(shared):{}),...values(block)};
  for(const state of ['warning','error','success','info','purple','pink'])assert.ok(contrast(tokens['fg-'+state],tokens['surface-'+state])>=4.5,`${theme}: ${state}`);
 }
 assert.doesNotMatch(css,/filter\s*:\s*(invert|hue-rotate)/);
});
