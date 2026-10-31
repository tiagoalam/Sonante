import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import {NowPlayingFullscreen} from '../src/utils/nowPlayingFullscreen.ts';
import * as layouts from '../src/utils/nowPlayingLayout.ts';
const flush=async()=>{for(let i=0;i<60;i++)await Promise.resolve()};
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
function compile(file,deps){const ex={};new Function('require','exports',ts.transpileModule(readFileSync(new URL('../src/'+file,import.meta.url),'utf8'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},ex);return ex;}

test('fullscreen suppresses duplicate requests, fences stale initial state, and reports only confirmed transitions',async()=>{
 const initial=deferred(),changed=[],failures=[],sets=[];let reads=0,actual=false;
 const controller=new NowPlayingFullscreen({window:{isFullscreen:()=>++reads===1?initial.promise:Promise.resolve(actual),setFullscreen:async value=>{sets.push(value);actual=value}},changed:value=>changed.push(value),failed:(...value)=>failures.push(value),restoreOnDispose:false});
 void controller.refresh();const first=controller.toggle();assert.equal(controller.toggle(),first);await first;
 assert.deepEqual(sets,[true]);assert.deepEqual(changed,[true]);initial.resolve(false);await flush();assert.deepEqual(changed,[true],'old initial query cannot undo a completed toggle');
 await controller.toggle();assert.deepEqual(sets,[true,false]);assert.deepEqual(changed,[true,false]);assert.deepEqual(failures,[]);controller.dispose();
});

test('embedded fullscreen cleanup restores only an entry owned by this host and handles native calls still in flight',async()=>{
 for(const restoreOnDispose of [true,false]){
  const entry=deferred(),sets=[],changed=[];
  const controller=new NowPlayingFullscreen({window:{isFullscreen:async()=>false,setFullscreen:value=>{sets.push(value);return value?entry.promise:Promise.resolve()}},changed:value=>changed.push(value),failed:()=>assert.fail('unexpected window error'),restoreOnDispose});
  const entering=controller.toggle();await flush();assert.deepEqual(sets,[true]);controller.dispose();controller.dispose();entry.resolve();await entering;await flush();
  assert.deepEqual(sets,restoreOnDispose?[true,false]:[true]);assert.deepEqual(changed,[],'unmounted host receives no state update');await controller.toggle();assert.deepEqual(sets,restoreOnDispose?[true,false]:[true]);
 }
 const sets=[];const preexisting=new NowPlayingFullscreen({window:{isFullscreen:async()=>true,setFullscreen:async value=>sets.push(value)},changed(){},failed(){assert.fail()},restoreOnDispose:true});await preexisting.refresh();preexisting.dispose();await flush();assert.deepEqual(sets,[],'fullscreen entered elsewhere is preserved');
 const query=deferred();const early=new NowPlayingFullscreen({window:{isFullscreen:()=>query.promise,setFullscreen:async()=>assert.fail('closed before native effect')},changed(){assert.fail('unmounted state')},failed(){assert.fail()},restoreOnDispose:true});const pending=early.toggle();early.dispose();query.resolve(false);await pending;await flush();
});

test('fullscreen failures do not publish success and explicit exit does not restore fullscreen later',async()=>{
 const changed=[],errors=[];let fail=true;const sets=[];
 const controller=new NowPlayingFullscreen({window:{isFullscreen:async()=>false,setFullscreen:async value=>{if(fail)throw Error('synthetic');sets.push(value)}},changed:value=>changed.push(value),failed:operation=>errors.push(operation),restoreOnDispose:true});
 await controller.toggle();assert.deepEqual(changed,[]);assert.deepEqual(errors,['toggle']);fail=false;await controller.toggle();await controller.exit();controller.dispose();await flush();assert.deepEqual(sets,[true,false]);assert.deepEqual(changed,[true,false]);
});

function fixture(embedded=true){
 const before={window:globalThis.window,document:globalThis.document};
 const slots=[],pending=[],listeners=new Map(),stored=new Map(),native=[],analyzer=[];let cursor=0,modal=false,actualFullscreen=false,collapse=0;
 const element=(tag='button',inside=true,editable=false,role='')=>({inside,isContentEditable:editable,closest:selector=>((['input','textarea','select'].includes(tag)&&selector.includes(tag))||(role&&selector.includes(`[role="${role}"]`)))?{}:null});
 const body=element('body',false),html=element('html',false),root={contains:element=>element.inside};
 globalThis.document={body,documentElement:html,activeElement:body,querySelector:selector=>selector==='.np-shell'?root:modal?{}:null,getElementById:()=>({focus(){}})};
 globalThis.window={crypto:{randomUUID:()=>`lease-${Math.random()}`},localStorage:{getItem:key=>stored.get(key)??null,setItem:(key,value)=>stored.set(key,value)},setTimeout,clearTimeout,addEventListener:(name,fn)=>{if(!listeners.has(name))listeners.set(name,new Set());listeners.get(name).add(fn)},removeEventListener:(name,fn)=>{listeners.get(name)?.delete(fn);if(!listeners.get(name)?.size)listeners.delete(name)}};
 const hooks={...React,default:React,useState:initial=>{const index=cursor++;if(!(index in slots))slots[index]=typeof initial==='function'?initial():initial;return[slots[index],value=>{slots[index]=typeof value==='function'?value(slots[index]):value}]},useRef:initial=>{const index=cursor++;return slots[index]??={current:initial}},useEffect:(fn,deps)=>{const index=cursor++,old=slots[index];if(!old||deps.some((value,i)=>!Object.is(value,old.deps[i])))pending.push(()=>{old?.cleanup?.();slots[index]={deps,cleanup:fn()}})}};
 const {NowPlayingHost}=compile('components/NowPlayingHost.tsx',{
  react:hooks,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:key=>key})},'../utils/nowPlayingFullscreen':{NowPlayingFullscreen},'../utils/nowPlayingLayout':layouts,
  '../utils/signalPath':{canToggleMute:()=>false},'./PlaybackOptionsControls':{PlaybackOptionsControls:()=>null},'./NowPlayingPresentation':{NowPlayingPresentation:()=>null},'./NowPlayingQueue':{NowPlayingQueue:()=>null},
  '../utils/frontendDiagnostics':{logError:()=>assert.fail('unexpected diagnostic')},'../services/audio':{audioService:new Proxy({},{get(){assert.fail('visual shortcut must not access audio controls')}})},
  '@tauri-apps/api/event':{listen:async()=>{analyzer.push('listen');return()=>analyzer.push('unlisten')}},'@tauri-apps/api/window':{getCurrentWindow:()=>({isFullscreen:async()=>{native.push('status');return actualFullscreen},setFullscreen:async value=>{native.push(value);actualFullscreen=value}})},
  '../services/nowPlayingWindow':{AUDIO_LEVEL_EVENT:'levels',ANALYZER_STATUS_EVENT:'analyzer',startAudioAnalyzer:async()=>analyzer.push('start'),stopAudioAnalyzer:async()=>analyzer.push('stop')},'../utils/nowPlayingHosting':{analyzerOwnerMatches:()=>true}
 });
 const owner={host:embedded?'main':'now-playing',sessionId:'fixture',generation:1};
 let props={snapshot:{health:{state:'available'},playback:{state:'play',elapsed:10,title:'Synthetic',artist:'Artist',current_media:{kind:'local'},volume:{backend:'unavailable'}}},owner,embedded,onReady(){},onCollapse(){collapse++}};
 const render=(change={})=>{props={...props,...change};cursor=0;const tree=NowPlayingHost(props);pending.splice(0).forEach(fn=>fn());return tree};
 const key=(key,extra={})=>{const event={key,repeat:false,isComposing:false,ctrlKey:false,altKey:false,metaKey:false,shiftKey:false,keyCode:0,defaultPrevented:false,target:body,preventDefault(){this.defaultPrevented=true},stopPropagation(){this.stopped=true},...extra};for(const fn of [...listeners.get('keydown')??[]])fn(event);return event};
 return{render,key,element,stored,analyzer,native,listeners,setModal:value=>{modal=value},get fullscreen(){return actualFullscreen},get collapse(){return collapse},async dispose(){slots.forEach(slot=>slot?.cleanup?.());await flush();globalThis.window=before.window;globalThis.document=before.document}};
}

test('actual host layout keys work in both windows and Local/Plex without restarting analyzer or touching playback',async()=>{
 for(const embedded of [true,false]){
  const f=fixture(embedded);
  try{
   let tree=f.render();await flush();tree=f.render();const native=[...f.native],analyzer=[...f.analyzer];
   for(const kind of ['local','plex']){
    tree=f.render({snapshot:{health:{state:'available'},playback:{state:'play',elapsed:10,title:'Synthetic',current_media:{kind},volume:{backend:'unavailable'}}}});
    for(const [index,layout]of layouts.NOW_PLAYING_LAYOUTS.entries()){
     const event=f.key(String(index+1));tree=f.render();assert.equal(tree.props.layout,layout);assert.ok(event.defaultPrevented&&event.stopped);
     if(layout!=='audiophile'||f.stored.has(layouts.NOW_PLAYING_LOOK_KEY))assert.equal(JSON.parse(f.stored.get(layouts.NOW_PLAYING_LOOK_KEY)).layout,layout);
    }
   }
   assert.deepEqual(f.native,native);assert.deepEqual(f.analyzer,analyzer,'layout keys preserve analyzer ownership and its initial listener lease');
  }finally{await f.dispose()}
  assert.equal(f.listeners.size,0);
 }
});

test('actual layout keys respect native fields, queue filters, dialogs, menus, modifiers, composition, repeats and handoff',async()=>{
 const f=fixture();
 try{
  let tree=f.render();await flush();tree=f.render();
  const ignored=[{repeat:true},{isComposing:true},{keyCode:229},{ctrlKey:true},{altKey:true},{metaKey:true},{shiftKey:true},{defaultPrevented:true},
   ...['input','textarea','select'].map(tag=>({target:f.element(tag)})),{target:f.element('div',true,true)},
   ...['textbox','combobox','slider','spinbutton'].map(role=>({target:f.element('div',true,false,role)})),{target:f.element('button',false)}];
  for(const extra of ignored){f.key('2',extra);assert.equal(f.render().props.layout,'audiophile')}
  for(const key of ['0','5','a']){assert.equal(f.key(key).defaultPrevented,false);assert.equal(f.render().props.layout,'audiophile')}
  f.setModal(true);assert.equal(f.key('3').defaultPrevented,false);assert.equal(f.render().props.layout,'audiophile');f.setModal(false);
  tree=f.render();tree.props.onSeekStart();tree=f.render();assert.equal(f.key('2').defaultPrevented,false);assert.equal(f.render().props.layout,'audiophile');
  // Leave the in-progress gesture intact; the handoff checks use a fresh host.
 }finally{await f.dispose()}
 const handoff=fixture();
 try{
  handoff.render();await flush();handoff.render();
  for(const change of [{busy:true},{hostingError:true},{owner:null},{owner:{host:'now-playing',sessionId:'fixture',generation:1}}]){
   handoff.render({busy:false,hostingError:false,owner:{host:'main',sessionId:'fixture',generation:1},...change});assert.equal(handoff.key('2').defaultPrevented,false);assert.equal(handoff.render().props.layout,'audiophile');
  }
 }finally{await handoff.dispose()}
});

test('embedded Escape closes queue, exits fullscreen, then collapses; leaving an owned fullscreen host restores main window',async()=>{
 const f=fixture();
 try{
  let tree=f.render();await flush();tree=f.render();assert.equal(tree.props.queueOpen,true);
  tree.props.onFullscreen();await flush();tree=f.render();assert.equal(tree.props.fullscreen,true);assert.ok(f.fullscreen);
  f.key('Escape');tree=f.render();assert.equal(tree.props.queueOpen,false);assert.equal(f.fullscreen,true);assert.equal(f.collapse,0);
  f.key('Escape');await flush();tree=f.render();assert.equal(tree.props.fullscreen,false);assert.equal(f.fullscreen,false);assert.equal(f.collapse,0);
  f.key('Escape');assert.equal(f.collapse,1);
  tree.props.onFullscreen();await flush();assert.equal(f.fullscreen,true);
 }finally{await f.dispose()}
 assert.equal(f.fullscreen,false,'collapsing/navigating/detaching must not strand the main window fullscreen');
});
