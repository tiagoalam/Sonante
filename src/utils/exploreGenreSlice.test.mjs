import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as gridMetrics from "./virtualGrid.ts";
import { createExploreSession } from "./explore.ts";

const modules={};
const hooks=Object.fromEntries(["useState","useRef","useCallback","useLayoutEffect","useImperativeHandle"].map(name=>[name,(...args)=>globalThis.__genreHost[name](...args)]));
const react={__esModule:true,...React,...hooks,default:React};
const compile=file=>ts.transpileModule(readFileSync(new URL(file,import.meta.url),"utf8"),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const load=(file,deps)=>{const exports={};new Function("require","exports",compile(file))(name=>{assert.ok(name in deps,name);return deps[name];},exports);return exports;};
modules.grid=load("../components/VirtualAlbumGrid.tsx",{react,"react/jsx-runtime":await import("react/jsx-runtime"),"../utils/virtualGrid":gridMetrics});
modules.slice=load("../components/ExploreGenreSlice.tsx",{react,"react/jsx-runtime":await import("react/jsx-runtime"),"./VirtualAlbumGrid":modules.grid});
const host=(Component,initial)=>{
  let cursor=0,props=initial;const slots=[],effects=[];
  const h={
    useState(value){const i=cursor++;if(!(i in slots))slots[i]=typeof value==="function"?value():value;return[slots[i],next=>{slots[i]=typeof next==="function"?next(slots[i]):next;}];},
    useRef(value){return slots[cursor++]??={current:value};},
    useCallback(fn,deps){const i=cursor++,old=slots[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))slots[i]={value:fn,deps};return slots[i].value;},
    useLayoutEffect(fn,deps){const i=cursor++,old=slots[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))effects.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()};});},
    useImperativeHandle(ref,fn){cursor++;h.handle=fn();},
    render(next=props){props=next;cursor=0;globalThis.__genreHost=h;return typeof Component==="function"?Component(props):Component.render(props,null);},
    effects(){effects.splice(0).forEach(fn=>fn());},close(){slots.forEach(v=>v?.cleanup?.());}
  };return h;
};
const nodes=(tree,predicate)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,predicate)):!tree||typeof tree!=="object"?[]:[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)];

test("actual genre viewport and shared virtual grid reach the last of thousands, restore after detail and keep position on additions",()=>{
  const old={ResizeObserver:globalThis.ResizeObserver,requestAnimationFrame:globalThis.requestAnimationFrame,cancelAnimationFrame:globalThis.cancelAnimationFrame};
  const observers=new Set(),frames=new Map();let frame=0;
  globalThis.ResizeObserver=class{constructor(fn){this.fn=fn;this.targets=new Set();observers.add(this);}observe(node){this.targets.add(node);}disconnect(){observers.delete(this);}};
  globalThis.requestAnimationFrame=fn=>{frames.set(++frame,fn);return frame;};globalThis.cancelAnimationFrame=id=>frames.delete(id);
  const albums=Array.from({length:5000},(_,i)=>({id:`fixture-${i}`,title:`Album ${i}`,artist:"Fixture artist",genres:["Reggae"],folder_path:"Synthetic",discs:[],track_count:1}));
  const last={...albums[0],id:"last",title:"Local last"};const c={local:{albums:[...albums,last],state:"ready"}};
  const session=createExploreSession(()=>0.4);session.syncContext(c,true);session.openSlice(session.getSnapshot().genres[0],session.getSnapshot().revision);
  const entries=session.getSnapshot().slice.entries,id=session.getSnapshot().slice.group.key,positions={[id]:1200},opened=[];
  const listeners=new Map();let offset=0,height=0;
  const viewport={clientWidth:800,clientHeight:700,get scrollTop(){return offset;},set scrollTop(value){offset=Math.max(0,Math.min(value,Math.max(0,height-this.clientHeight)));},getBoundingClientRect(){return{top:0};},
    addEventListener(name,fn){const set=listeners.get(name)??new Set();set.add(fn);listeners.set(name,set);},removeEventListener(name,fn){listeners.get(name)?.delete(fn);}};
  const content={get scrollHeight(){return height;}};
  const gridNode={clientWidth:800,getBoundingClientRect(){return{top:-offset};}};
  let rendered=[];
  const props={id,label:"Reggae",entries,active:true,positions,renderItem:entry=>{rendered.push(entry);return React.createElement("button",{onClick:()=>{assert.equal(session.canUseSlice(entry,session.getSnapshot().revision),true);opened.push(entry);}},entry.album.title);}};
  const slice=host(modules.slice.ExploreGenreSlice,props),grid=host(modules.grid.VirtualAlbumGrid,{});let tree,current=props;
  const notify=()=>{for(const observer of observers)observer.fn([]);for(const[id,fn]of [...frames]){frames.delete(id);fn();}};
  const render=(next=current)=>{current=next;rendered=[];for(let i=0;i<4;i++){
    const root=slice.render(current);root.props.ref(viewport);root.props.children.props.ref(content);slice.effects();
    const element=nodes(root,n=>n.type===modules.grid.VirtualAlbumGrid)[0];tree=grid.render(element.props);tree.props.ref.current=gridNode;
    height=tree.props.style?.height??0;grid.effects();notify();
  }return tree;};
  const scroll=top=>{viewport.scrollTop=top;for(const fn of listeners.get("scroll")??[])fn();notify();};
  try{
    render();assert.equal(viewport.scrollTop,1200);assert.equal(positions[id],1200);assert.ok(nodes(tree,n=>n.type==="button").length<60);assert.ok(rendered.length<250,"bounded renderItem invocations, not 5001 cards");
    scroll(height);render();const buttons=nodes(tree,n=>n.type==="button");assert.equal(buttons.at(-1).props.children,"Local last");assert.ok(buttons.length<60);buttons.at(-1).props.onClick();assert.equal(opened[0],entries.at(-1));assert.equal(opened[0].source,"local");
    const saved=positions[id];render({...props,active:false});viewport.clientHeight=0;viewport.scrollTop=0;render({...props,active:false});viewport.clientHeight=700;render(props);assert.equal(viewport.scrollTop,saved);
    const extra=Array.from({length:20},(_,i)=>({...c.local.albums[0],id:`more-${i}`,title:`More ${i}`}));
    const next={local:{...c.local,albums:[...c.local.albums,...extra]}};session.syncContext(next,true);
    render({...props,entries:session.getSnapshot().slice.entries});assert.equal(viewport.scrollTop,saved);assert.equal(positions[id],saved);
    scroll(height);render();assert.equal(nodes(tree,n=>n.type==="button").at(-1).props.children,"More 19");assert.equal(session.getSnapshot().slice.entries.length,5021);
    const before=session.getSnapshot();session.setPage(1,before.revision);assert.equal(session.getSnapshot(),before,"genre has no page state transition");
  }finally{slice.close();grid.close();Object.assign(globalThis,old);}
});
