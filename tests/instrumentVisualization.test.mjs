import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import {InstrumentMotion,levelDbfs,meterPosition} from '../src/utils/instrumentMotion.ts';
const read=file=>readFileSync(new URL('../src/'+file,import.meta.url),'utf8');
function compile(file,deps){const out={};new Function('require','exports',ts.transpileModule(read(file),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText)(key=>{assert.ok(key in deps,key);return deps[key]},out);return out;}
const drawing=compile('utils/instrumentDrawing.ts',{'./instrumentMotion':{INSTRUMENT_BANDS:48,levelDbfs,meterPosition}});
const frame=(value,extra={})=>({leftRms:value,rightRms:value,leftPeak:value,rightPeak:value,spectrum:Array(48).fill(value),...extra});

test('instrument attacks reach current RMS and spectrum together within two 60Hz frames without frame-rate-dependent lag',()=>{
 for(const hz of [30,60,120]){
  const motion=new InstrumentMotion();motion.setTarget(frame(.8),true);
  let elapsed=0;while(elapsed<1000/30-.001){motion.step(1000/hz);elapsed+=1000/hz;}
  assert.ok(motion.frame.rms[0]>.8*.95);assert.equal(motion.frame.rms[0],motion.frame.spectrum[10]);
  assert.equal(motion.frame.peak[0],.8,'new peaks have no animated attack delay');
 }
});
test('latest analyzer packet replaces older targets and display never mutates telemetry or invents stale peaks',()=>{
 const m=new InstrumentMotion();const old=frame(1),latest=frame(.2);m.setTarget(old,true);m.setTarget(latest,true);m.step(1000/60);
 assert.ok(m.frame.rms[0]<.2);assert.equal(m.frame.peak[0],.2);assert.deepEqual(old,frame(1));assert.deepEqual(latest,frame(.2));
 m.setTarget(frame(NaN,{leftPeak:Infinity,rightPeak:-1,spectrum:[NaN,Infinity,-1]}),false);for(let i=0;i<150;i++)m.step(16.67);
 assert.ok([...m.frame.rms,...m.frame.peak,...m.frame.spectrum,...m.frame.spectrumPeaks].every(v=>v===0));
});
test('RMS and peak stay distinct per channel, digital zero is bounded and silence is not a positive measurement',()=>{
 assert.equal(levelDbfs(1),0);assert.ok(Math.abs(levelDbfs(.5)+6.0206)<.0001);assert.equal(meterPosition(0),0);assert.equal(meterPosition(NaN),0);
 const m=new InstrumentMotion();m.setTarget(frame(.2,{rightRms:.4,leftPeak:.5,rightPeak:.8}),true);m.step(16.67,true);
 assert.deepEqual(m.frame.rms,[.2,.4]);assert.deepEqual(m.frame.peak,[.5,.8]);
});
test('cached instrument faces and all dynamic modes draw finite bounded geometry with honest dBFS scales',()=>{
 const calls=[],gradient={addColorStop(){}};
 const ctx=new Proxy({createLinearGradient:()=>gradient,createRadialGradient:()=>gradient},{get:(obj,key)=>obj[key]??((...args)=>calls.push([key,...args])),set:(obj,key,value)=>{obj[key]=value;return true}});
 const m=new InstrumentMotion();m.setTarget(frame(.4),true);m.step(16.67);
 for(const mode of ['vu-amber','vu-blue','vu-radial','combo','spectrum-neon','spectrum-segmented','levels'])for(const [w,h] of [[600,180],[240,100]]){
  calls.length=0;drawing.drawInstrumentBackground(ctx,w,h,mode,{left:'LEFT',right:'RIGHT',relative:'Relative'});drawing.drawInstrumentForeground(ctx,w,h,mode,m.frame);
  assert.ok(calls.length<800);for(const[, ...args]of calls)for(const n of args)if(typeof n==='number')assert.ok(Number.isFinite(n));
  if(mode==='vu-amber'||mode==='vu-blue')assert.ok(calls.some(([name,text])=>name==='fillText'&&text==='RMS · dBFS'));
 }
 assert.deepEqual(drawing.instrumentCanvasSize(7680,4320,3),{width:1422,height:800,ratio:800/4320,logicalWidth:7680,logicalHeight:4320});
});

test('real combo has one animation loop, uses the latest frame, caches its face and releases hidden/unmounted work',()=>{
 const before={window:globalThis.window,document:globalThis.document,resize:globalThis.ResizeObserver,intersection:globalThis.IntersectionObserver};
 try{
  const raf=new Map(),docEvents=new Map(),motionEvents=new Map(),slots=[],pending=[],draws=[];let nextId=0,cursor=0,faces=0,resize,intersect;
  const canvas={clientWidth:600,clientHeight:180,width:0,height:0,getContext:()=>ctx};const ctx={setTransform(){},drawImage(){}};
  const motion={matches:false,addEventListener:(key,fn)=>motionEvents.set(key,fn),removeEventListener:key=>motionEvents.delete(key)};
  globalThis.window={devicePixelRatio:2,matchMedia:()=>motion,requestAnimationFrame:fn=>{raf.set(++nextId,fn);return nextId},cancelAnimationFrame:id=>raf.delete(id)};
  globalThis.document={hidden:false,createElement:()=>{faces++;return{width:0,height:0,getContext:()=>ctx}},addEventListener:(key,fn)=>docEvents.set(key,fn),removeEventListener:key=>docEvents.delete(key)};
  globalThis.ResizeObserver=class{constructor(fn){resize=fn}observe(){}disconnect(){resize=null}};
  globalThis.IntersectionObserver=class{constructor(fn){intersect=fn}observe(){}disconnect(){intersect=null}};
  const hooks={...React,default:React,useRef:initial=>{const index=cursor++;return slots[index]??={current:initial}},useEffect:(fn,deps)=>{const index=cursor++,old=slots[index];if(!old||deps.some((value,i)=>!Object.is(value,old.deps[i])))pending.push(()=>{old?.cleanup?.();slots[index]={deps,cleanup:fn()}})}};
  const {InstrumentCanvas}=compile('components/visualizer/InstrumentCanvas.tsx',{react:hooks,'react/jsx-runtime':jsx,'../../utils/instrumentMotion':{InstrumentMotion},'../../utils/instrumentDrawing':{...drawing,drawInstrumentBackground(){},drawInstrumentForeground:(_ctx,_w,_h,mode,value)=>draws.push({mode,rms:value.rms[0],spectrum:value.spectrum[0]})}});
  const render=(levels)=>{cursor=0;const tree=InstrumentCanvas({levels,active:true,mode:'combo',label:'Combo'});tree.props.ref.current=canvas;pending.splice(0).forEach(fn=>fn());return tree;};
  const tick=time=>{for(const [id,fn]of [...raf]){raf.delete(id);fn(time)}};
  render(frame(1));render(frame(.4));assert.equal(raf.size,1);tick(1);assert.equal(draws.length,1);assert.equal(draws[0].rms,draws[0].spectrum);assert.ok(draws[0].rms<.4);assert.equal(faces,1);
  tick(18);assert.equal(faces,1);assert.ok(draws.at(-1).rms>.4*.95);
  document.hidden=true;docEvents.get('visibilitychange')();assert.equal(raf.size,0);render(frame(.6));assert.equal(raf.size,0);
  document.hidden=false;docEvents.get('visibilitychange')();tick(60);assert.equal(faces,1);
  intersect([{isIntersecting:false}]);assert.equal(raf.size,0);intersect([{isIntersecting:true}]);assert.equal(raf.size,1);
  motion.matches=true;motionEvents.get('change')();tick(100);assert.equal(raf.size,0);assert.equal(draws.at(-1).rms,.6);
  canvas.clientWidth=500;resize();tick(120);assert.equal(faces,2,'static face rebuilt only for resize/mode/labels');
  motion.matches=false;motionEvents.get('change')();const count=draws.length;
  for(let i=0;i<60;i++){render(frame(i%2?.3:.6));tick(200+i*1000/60);}
  assert.ok(draws.length-count>=58,'floating-point RAF timestamps must not halve the drawing cadence');
  for(const slot of slots)slot?.cleanup?.();assert.equal(raf.size,0);assert.equal(docEvents.size,0);assert.equal(motionEvents.size,0);assert.equal(resize,null);assert.equal(intersect,null);
 }finally{globalThis.window=before.window;globalThis.document=before.document;globalThis.ResizeObserver=before.resize;globalThis.IntersectionObserver=before.intersection;}
});
