import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import {NowPlayingIdle} from '../src/utils/nowPlayingIdle.ts';
import {normalizeQueuePanels,parseQueuePanels,readQueuePanels,saveQueuePanels,NOW_PLAYING_QUEUE_PANELS_KEY} from '../src/utils/nowPlayingLayout.ts';
function clock(){let time=0,id=0;const pending=new Map();return{pending,set(fn,ms){pending.set(++id,{fn,due:time+ms});return id},clear(id){pending.delete(id)},advance(ms){const end=time+ms;for(;;){const entry=[...pending].sort((a,b)=>a[1].due-b[1].due).find(([,job])=>job.due<=end);if(!entry)break;pending.delete(entry[0]);time=entry[1].due;entry[1].fn()}time=end}}}
function compile(file,deps){const out={};new Function('require','exports',ts.transpileModule(readFileSync(new URL('../src/'+file,import.meta.url),'utf8'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText)(key=>{assert.ok(key in deps,key);return deps[key]},out);return out;}
function events(){const map=new Map();return{map,add(name,fn,options){map.set(name+(options===true?':capture':''),fn)},remove(name,fn,options){const key=name+(options===true?':capture':'');if(map.get(key)===fn)map.delete(key)}}}

test('idle has one cancelable deadline, reveals on activity, blocks interactions without polling and disposes stale callbacks',()=>{
 const c=clock(),states=[];let blocked=false;
 const idle=new NowPlayingIdle({setTimer:c.set,clearTimer:c.clear,changed:value=>states.push(value),blocked:()=>blocked});
 idle.activity();c.advance(3999);assert.equal(idle.hidden,false);idle.activity();assert.equal(c.pending.size,1);c.advance(4000);assert.equal(idle.hidden,true);
 idle.activity();assert.deepEqual(states,[true,false]);blocked=true;c.advance(4000);assert.equal(idle.hidden,false);assert.equal(c.pending.size,0,'no polling while interaction remains blocked');
 blocked=false;idle.activity();const stale=[...c.pending.values()][0].fn;idle.dispose();stale();assert.equal(idle.hidden,false);assert.equal(c.pending.size,0);assert.deepEqual(states,[true,false]);
});

test('actual immersive frame survives telemetry rerenders and protects pointer, keyboard, popover and window lifecycle',()=>{
 const oldWindow=globalThis.window,oldDocument=globalThis.document;
 try{
  const c=clock(),win=events(),doc=events(),rootEvents=events(),raf=new Map(),attrs=new Map(),slots=[],pending=[];let cursor=0,id=0,open=false,focusCount=0;
  const body={matches:()=>false},button={matches:()=>false,focus(){focusCount++;document.activeElement=button}};
  const root={contains:node=>node===button,querySelector:selector=>selector.includes('.np-header')?button:open?{}:null,removeAttribute:key=>attrs.delete(key),addEventListener:rootEvents.add,removeEventListener:rootEvents.remove};
  globalThis.window={setTimeout:c.set,clearTimeout:c.clear,requestAnimationFrame:fn=>{raf.set(++id,fn);return id},cancelAnimationFrame:id=>raf.delete(id),addEventListener:win.add,removeEventListener:win.remove};
  globalThis.document={hidden:false,activeElement:body,addEventListener:doc.add,removeEventListener:doc.remove};
  const hooks={...React,default:React,useRef:initial=>{const index=cursor++;return slots[index]??={current:initial}},useState:initial=>{const index=cursor++;if(!(index in slots))slots[index]=initial;return[slots[index],next=>{slots[index]=next}]},useEffect:(fn,deps)=>{const index=cursor++,previous=slots[index];if(!previous||deps.some((v,i)=>!Object.is(v,previous.deps[i])))pending.push(()=>{previous?.cleanup?.();slots[index]={deps,cleanup:fn()}})}};
  const {NowPlayingImmersive}=compile('components/NowPlayingImmersive.tsx',{react:hooks,'react/jsx-runtime':jsx,'../utils/nowPlayingIdle':{NowPlayingIdle}});
  const child=React.createElement('section',null,'Retained cover, instruments and queue');
  const render=(enabled=true)=>{cursor=0;const tree=NowPlayingImmersive({enabled,className:'np-shell',children:child});tree.props.ref.current=root;if(tree.props['data-np-controls-hidden'])attrs.set('data-np-controls-hidden','true');else attrs.delete('data-np-controls-hidden');pending.splice(0).forEach(fn=>fn());return tree};
  let tree=render();assert.equal(tree.props.children,child);assert.equal(c.pending.size,1);
  c.advance(2000);for(let i=0;i<50;i++)render();c.advance(2000);tree=render();assert.equal(tree.props['data-np-controls-hidden'],'true','telemetry/elapsed rerenders never reset the timer');
  win.map.get('pointermove')({buttons:0});assert.equal(attrs.has('data-np-controls-hidden'),false,'reveal is synchronous for Escape focus restoration');render();assert.equal(c.pending.size,1);
  c.advance(4000);render();let prevented=false;win.map.get('keydown:capture')({key:'Tab',preventDefault(){prevented=true}});assert.ok(prevented);assert.equal(raf.size,1);for(const[id,fn]of [...raf]){raf.delete(id);fn()}assert.equal(focusCount,1);render();c.advance(4000);assert.equal(render().props['data-np-controls-hidden'],undefined,'keyboard focus keeps controls accessible');
  document.activeElement=body;rootEvents.map.get('focusout')();c.advance(4000);assert.equal(render().props['data-np-controls-hidden'],'true');
  win.map.get('pointerdown')();render();c.advance(4000);assert.equal(render().props['data-np-controls-hidden'],undefined,'dragging never fades controls');win.map.get('pointermove')({buttons:0});c.advance(4000);assert.equal(render().props['data-np-controls-hidden'],'true');
  open=true;rootEvents.map.get('toggle:capture')();render();c.advance(4000);assert.equal(render().props['data-np-controls-hidden'],undefined,'open menus/help/error block hiding');open=false;rootEvents.map.get('toggle:capture')();c.advance(4000);assert.equal(render().props['data-np-controls-hidden'],'true');
  win.map.get('blur')();assert.equal(c.pending.size,0);assert.equal(render().props['data-np-controls-hidden'],undefined);win.map.get('focus')();assert.equal(c.pending.size,1);
  document.hidden=true;doc.map.get('visibilitychange')();assert.equal(c.pending.size,0);document.hidden=false;doc.map.get('visibilitychange')();assert.equal(c.pending.size,1);
  assert.equal(render(false).props['data-np-controls-hidden'],undefined);assert.equal(c.pending.size,0);assert.equal(raf.size,0);assert.equal(win.map.size,0);assert.equal(doc.map.size,0);assert.equal(rootEvents.map.size,0);
 }finally{globalThis.window=oldWindow;globalThis.document=oldDocument;}
});

test('queue panel defaults preserve prior behavior and malformed/blocked storage cannot prevent opening',()=>{
 const defaults={audiophile:true,psycho:true,sleeve:true,poster:true};
 for(const value of [null,'broken','[]','null','{"psycho":"false","audiophile":3}'])assert.deepEqual(parseQueuePanels(value),defaults);
 assert.deepEqual(parseQueuePanels('{"psycho":false,"poster":false}'),{...defaults,psycho:false,poster:false});
 const oldWindow=globalThis.window,stored=new Map();
 try{
  globalThis.window={localStorage:{getItem:key=>stored.get(key)??null,setItem:(key,value)=>stored.set(key,value)}};
  const changed={...defaults,psycho:false};saveQueuePanels(changed);assert.deepEqual(readQueuePanels(),changed);assert.ok(stored.has(NOW_PLAYING_QUEUE_PANELS_KEY));
  globalThis.window={localStorage:{getItem(){throw Error('blocked')},setItem(){throw Error('blocked')}}};assert.deepEqual(readQueuePanels(),defaults);assert.doesNotThrow(()=>saveQueuePanels(normalizeQueuePanels(null)));
 }finally{globalThis.window=oldWindow;}
});
