import { passiveQueueInsertion } from "./support/passiveQueueInsertion.mjs";
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import ts from 'typescript';
import * as model from '../src/utils/queuePresentation.ts';
import * as presentation from '../src/utils/nowPlayingPresentation.ts';
const read = path => readFileSync(new URL('../'+path,import.meta.url),'utf8');
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
const flush=async()=>{for(let n=0;n<25;n++)await Promise.resolve();};
const local=(index,extra={})=>({title:'Track '+index,artist:'Artist',album:'Album',duration:100,media_locator:{kind:'local',uri:'item-'+index},...extra});
const compile=(file,deps)=>{const source=ts.transpileModule(read(file),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const exports={};new Function('require','exports',source)(key=>{if(key in deps)return deps[key];if(key in passiveQueueInsertion)return passiveQueueInsertion[key];assert.ok(false,key);},exports);return exports;};
const dictionary=language=>{const data=JSON.parse(read('src/locales/'+language+'.json'));return(key,args={})=>{const get=k=>k.split('.').reduce((v,p)=>v?.[p],data);const value=(args.count!==undefined?get(key+(args.count===1?'_one':'_other')):null)??get(key)??key;return value.replace(/\{\{(\w+)\}\}/g,(_m,k)=>String(args[k]??''));};};
function fixture(items,language='en') {
 let cursor=0;const slots=[],layouts=[],calls=[],artworkScopes=[];const same=(a,b)=>a?.length===b?.length&&a.every((v,n)=>Object.is(v,b[n]));
 const hooks={...React,useId:()=> 'queue-owned-filter',useEffect(){},useState(initial){const n=cursor++;slots[n]??={value:n===0?{items,phase:'ready'}:initial};return[slots[n].value,value=>slots[n].value=typeof value==='function'?value(slots[n].value):value];},useRef(initial){return slots[cursor++]??={current:initial};},useMemo(fn,deps){const n=cursor++;if(!same(slots[n]?.deps,deps))slots[n]={deps,value:fn()};return slots[n].value;},useLayoutEffect(fn,deps){const n=cursor++;if(!same(slots[n]?.deps,deps)){slots[n]={deps};layouts.push(fn);}}};
 const t=dictionary(language);const viewport={scrollTop:0,clientHeight:128};
 const f={backendItems:items,props:{open:true,available:true,status:{state:'play',elapsed:25,duration:100,options:{random:false,repeat:false,single:'off'},current_media:{kind:'local',uri:items[0]?.media_locator?.uri,queue_index:0},is_updating:false}},
  requests:[], snapshot:null, editFailure:null, paint(){for(let n=0;n<2;n++){cursor=0;f.tree=View(f.props);nodes(f.tree,n=>n.props?.className==='np-queue-scroll')[0].props.ref.current=viewport;layouts.splice(0).forEach(run=>run());}return f.tree;},
  filter(text){nodes(f.tree,n=>n.type==='input')[0].props.onChange({target:{value:text}});return f.paint();},
  rows(){return nodes(f.tree,n=>n.props?.['data-queue-index']!==undefined);},calls,artworkScopes,viewport,t};
 const deps={'./QueueInsertionMenu':{useQueueInsertion:()=>({open:(_event,request)=>f.requests.push(request),trigger:()=>null,menu:null,notice:null})},react:hooks,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t})},'lucide-react':icons,'../utils/queuePresentation':model,'../utils/nowPlayingPresentation':presentation,'./TrackArtwork':{TrackArtwork:()=>null},
  '../utils/playbackIntent':{beginPlaybackIntent:()=>({current:()=>true,confirmCurrent:async()=>true}),isPlaybackSuperseded:()=>false},
  '../utils/useNavidromeQueueArtwork':{useNavidromeQueueArtwork:()=>item=>item},
  '../utils/useLocalQueueArtwork':{useLocalQueueArtwork:(...args)=>{artworkScopes.push(args[8]);return item=>item;}},
  '../services/audio':{audioService:{getQueueEditSnapshot:async()=>{calls.push('snapshot');return f.snapshot ? await f.snapshot : {tracks:f.backendItems,revision:'synthetic-revision'};},editQueue:async(...args)=>{calls.push(['edit',...args]);if(f.editFailure)throw f.editFailure;},getQueue:async()=>{calls.push('read');return f.backendItems;},playQueueIndex:async index=>calls.push(['play',index]),clearQueue:async()=>calls.push('clear')}}};
 const View=compile('src/components/NowPlayingQueue.tsx',deps).NowPlayingQueue;f.paint();return f;
}

test('filtered queue clicks retain original indices and duplicates; filtering and jumping issue no transport',async()=>{
 const same=local('duplicate');const items=[same,local(1,{artist:'Other'}),same];const f=fixture(items);
 f.filter('duplicate');assert.deepEqual(f.rows().map(n=>n.props['data-queue-index']),[0,2]);assert.deepEqual(f.calls,[]);
 nodes(f.rows()[1],n=>n.props?.className==='np-queue-track')[0].props.onClick({});await flush();assert.deepEqual(f.calls,['read',['play',2]]);
 assert.deepEqual(items,[same,local(1,{artist:'Other'}),same]);
});

test('jump clears a hiding filter and scrolls the virtual list to the confirmed current occurrence',()=>{
 const items=Array.from({length:1500},(_,i)=>local(i));const f=fixture(items);
 f.props.status={...f.props.status,current_media:{kind:'local',uri:'item-1200',queue_index:1200}};
 f.paint();f.filter('Track 4');assert.ok(!f.rows().some(row=>row.props['data-queue-index']===1200));
 nodes(f.tree,n=>n.type==='button'&&n.props.children?.includes?.(f.t('queue.jumpToCurrent')))[0].props.onClick();f.paint();
 assert.equal(nodes(f.tree,n=>n.type==='input')[0].props.value,'');assert.ok(f.viewport.scrollTop>1100*64);
 const row=f.rows().find(row=>row.props['data-queue-index']===1200);assert.ok(row);assert.equal(row.props['aria-current'],'true');
 assert.ok(f.rows().length<=12,'virtualization stays bounded');assert.deepEqual(f.calls,[]);
 assert.ok(f.artworkScopes.at(-1).includes(1200));
});

test('filtered playback still rejects a changed full queue and unavailable playback',async()=>{
 const items=[local(0),local(1)];const f=fixture(items);f.filter('Track 1');f.backendItems=[items[1],items[0]];
 nodes(f.rows()[0],n=>n.props?.className==='np-queue-track')[0].props.onClick({});await flush();f.paint();
 assert.deepEqual(f.calls,['read']);assert.equal(nodes(f.tree,n=>n.props?.role==='alert').length,1);
 f.props.available=false;f.paint();const button=nodes(f.rows()[0],n=>n.props?.className==='np-queue-track')[0];assert.equal(button.props.disabled,true);
 button.props.onClick({});await flush();assert.deepEqual(f.calls,['read']);
});

test('duration labels remain full-queue estimates and clear control explicitly clears the entire filtered queue',async()=>{
 for(const lang of ['en','pt-BR']){
  const f=fixture([local(0),local(1),local(2)],lang);f.filter('Track 1');
  const text=nodes(f.tree,n=>n.type==='span').map(n=>n.props.children).filter(n=>typeof n==='string').join(' ');
  assert.ok(text.includes(f.t('queue.totalTime',{time:'5:00'})));assert.ok(text.includes(f.t('queue.remainingTime',{time:'4:35'})));
  assert.ok(text.includes(f.t('queue.filteredCount',{count:1,total:3})));
  const clear=nodes(f.tree,n=>n.type==='button'&&n.props['aria-label']===f.t('queue.clearAll'))[0];assert.ok(clear);clear.props.onClick();await flush();assert.deepEqual(f.calls,['clear']);
 }
});

test('Escape clears only the queue filter first and native composition retains its own keys',()=>{
 const f=fixture([local(0)]);f.filter('Track');let prevented=0,stopped=0;
 let field=nodes(f.tree,n=>n.type==='input')[0];field.props.onKeyDown({key:'Escape',nativeEvent:{isComposing:true},preventDefault:()=>prevented++,stopPropagation:()=>stopped++});f.paint();assert.equal(nodes(f.tree,n=>n.type==='input')[0].props.value,'Track');
 field.props.onKeyDown({key:'Escape',nativeEvent:{isComposing:false},preventDefault:()=>prevented++,stopPropagation:()=>stopped++});f.paint();assert.equal(nodes(f.tree,n=>n.type==='input')[0].props.value,'');assert.equal(prevented,1);assert.equal(stopped,1);
 nodes(f.tree,n=>n.type==='input')[0].props.onKeyDown({key:'Escape',nativeEvent:{isComposing:false},preventDefault:()=>assert.fail(),stopPropagation:()=>assert.fail()});
});

test('search focus styling uses native focus, with an opaque highlight and outline independent of focus-visible',()=>{
 const css=read('src/index.css');const rule=css.split('[data-unified-search] [data-search-result]:focus {')[1]?.split('}')[0];assert.ok(rule);assert.match(rule,/background-color: var\(--theme-surface-accent/);assert.match(rule,/outline: 2px solid var\(--theme-border-accent/);
});

test('shortcut help is bilingual, modal, keyboard-owned and returns focus without audio calls',()=>{
 for(const lang of ['en','pt-BR']){
  const t=dictionary(lang);let opened=false;const effect=[],calls=[];const trigger={isConnected:true,focus:()=>calls.push('focus')};const modal={open:false,showModal(){this.open=true;calls.push('open');},close(){this.open=false;calls.push('close');}};let refIndex=0;
  const hooks={...React,useId:()=> 'help-owned',useState:()=>[opened,value=>opened=value],useRef:()=>({current:refIndex++===0?modal:trigger}),useEffect:fn=>effect.push(fn)};
  const View=compile('src/components/KeyboardShortcutsHelp.tsx',{react:hooks,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t})}}).KeyboardShortcutsHelp;
  let tree=View();let stopped=0;nodes(tree,n=>n.type==='button')[0].props.onClick({stopPropagation:()=>stopped++});assert.equal(opened,true);effect.length=0;refIndex=0;tree=View();
  const cleanup=effect[0]();assert.deepEqual(calls,['open']);const dialog=nodes(tree,n=>n.type==='dialog')[0];assert.equal(dialog.props['aria-modal'],'true');dialog.props.onKeyDown({stopPropagation:()=>stopped++});dialog.props.onCancel({preventDefault(){}});assert.equal(opened,false);cleanup();assert.deepEqual(calls,['open','close','focus']);assert.equal(stopped,2);
  assert.equal(nodes(tree,n=>n.type==='kbd').length,10);assert.ok(nodes(tree,n=>n.type==='h2').some(n=>n.props.children===t('shortcuts.title')));
 }
 assert.match(read('src/App.tsx'),/<KeyboardShortcutsHelp \/>/);assert.doesNotMatch(read('src/components/KeyboardShortcutsHelp.tsx'),/audioService|invoke\(|fetch\(/);
});

test('queue keyboard edits and filtered drag use exact full indices and a captured revision, with no transport or replacement', async()=>{
 const items=[local('duplicate'),local(1),local('duplicate')];const f=fixture(items);f.filter('duplicate');
 const handle=nodes(f.rows()[1],n=>n.props?.className==='np-queue-drag')[0];
 handle.props.onKeyDown({altKey:true,key:'ArrowUp',preventDefault(){}});await flush();assert.equal(f.calls[0],'snapshot');assert.deepEqual(f.calls[1].slice(0,3),['edit','synthetic-revision',{kind:'move',from:2,to:1}]);
 f.calls.length=0;const transfer={setData(){},effectAllowed:''};handle.props.onDragStart({dataTransfer:transfer});await flush();
 f.rows()[0].props.onDrop({preventDefault(){}});await flush();assert.equal(f.calls[0],'snapshot');assert.deepEqual(f.calls[1].slice(0,3),['edit','synthetic-revision',{kind:'move',from:2,to:0}]);
 assert.equal(f.calls.length,2);assert.deepEqual(items,[local('duplicate'),local(1),local('duplicate')]);
});

test('drag cannot act after navigation/close, a changed preview or an external drop; current removal stays protected',async()=>{
 const items=[local(0),local(1)];
 for(const mode of ['close','changed','external']) {
  const f=fixture(items);let resolve;f.snapshot=new Promise(r=>resolve=r);
  if(mode!=='external')nodes(f.rows()[1],n=>n.props?.className==='np-queue-drag')[0].props.onDragStart({dataTransfer:{setData(){}}});
  if(mode==='close'){f.props.open=false;f.paint()}
  if(mode==='changed')f.backendItems=[items[1],items[0]];
  f.rows()[0]?.props.onDrop({preventDefault(){}});resolve({tracks:f.backendItems,revision:'changed'});await flush();assert.equal(f.calls.some(call=>Array.isArray(call)&&call[0]==='edit'),false,mode);
 }
 const f=fixture(items);f.rows()[0].props.onContextMenu({});let menu=f.requests.at(-1);assert.equal(menu.actions.find(a=>a.id==='remove').disabled,true);
 f.props.status={...f.props.status,state:'pause'};f.paint();f.rows()[0].props.onContextMenu({});assert.equal(f.requests.at(-1).actions.find(a=>a.id==='remove').disabled,true);
 f.rows()[1].props.onContextMenu({});menu=f.requests.at(-1);await menu.actions.find(a=>a.id==='remove').run({current:()=>true,confirmCurrent:async()=>true});assert.deepEqual(f.calls[1].slice(0,3),['edit','synthetic-revision',{kind:'remove',index:1}]);
});
