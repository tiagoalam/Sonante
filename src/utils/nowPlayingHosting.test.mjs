import test from 'node:test';
import assert from 'node:assert/strict';
import { NowPlayingHosting, analyzerOwnerMatches, capturePagePresentation } from './nowPlayingHosting.ts';
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<16;i++)await Promise.resolve();};
const fixture=(overrides={})=>{
 const calls=[];let generation=0,session=0;let coordinator;
 const services={claim:async(host,id)=>{calls.push(['claim',host,id]);return {host,sessionId:id,generation:++generation};},open:async()=>{calls.push(['open']);return 'runtime-one';},activate:async(runtime,owner)=>{calls.push(['activate',runtime,owner.generation]);},suspend:async()=>{calls.push(['suspend']);},close:async()=>{calls.push(['close']);},stop:async owner=>{calls.push(['stop',owner.generation]);},id:()=>String(++session),beforeEmbed:()=>calls.push(['capture']),...overrides};
 coordinator=new NowPlayingHosting(services);
 coordinator.subscribe(()=>{const s=coordinator.getSnapshot();if(s.mode==='embedded'&&s.owner)queueMicrotask(()=>coordinator.hostReady(s.owner));});
 return {coordinator,calls};
};
test('show/collapse restores presentation; no transport or queue operations; duplicate open is blocked',async()=>{
 const gate=deferred();const {coordinator:c,calls}=fixture({claim:async(host,id)=>{await gate.promise;return {host,sessionId:id,generation:1};}});
 const first=c.show();await c.show();assert.equal(c.getSnapshot().busy,true);gate.resolve();await first;
 assert.equal(c.getSnapshot().mode,'embedded');assert.deepEqual(calls,[['capture']]);await c.collapse();assert.equal(c.getSnapshot().mode,'closed');assert.deepEqual(calls.at(-1),['stop',1]);
});
test('detach keeps embedded until both readiness/activation; reincorporate is confirmed before close',async()=>{
 const open=deferred(),activation=deferred();const {coordinator:c,calls}=fixture({open:()=>open.promise,activate:()=>activation.promise});
 await c.show();const detach=c.detach();assert.equal(c.getSnapshot().mode,'embedded');assert.equal(c.getSnapshot().busy,true);
 await c.detach();open.resolve('runtime-one');await flush();assert.equal(c.getSnapshot().mode,'embedded');activation.resolve();await detach;
 assert.equal(c.getSnapshot().mode,'detached');assert.equal(c.isDetachedRuntime('runtime-old'),false);assert.equal(c.isDetachedRuntime('runtime-one'),true);
 await c.reincorporate();assert.equal(c.getSnapshot().mode,'embedded');assert.equal(c.getSnapshot().busy,false);assert.equal(calls.at(-1)[0],'close');
});
test('opening failure keeps the approved embedded host and exposes an error',async()=>{
 const {coordinator:c}=fixture({open:async()=>{throw Error('private message');}});await c.show();const owner=c.getSnapshot().owner;await c.detach();
 assert.equal(c.getSnapshot().mode,'embedded');assert.equal(c.getSnapshot().owner,owner);assert.equal(c.getSnapshot().error,true);assert.doesNotMatch(JSON.stringify(c.getSnapshot()),/private/);
});
test('activation failure reserves a new main generation; old readiness cannot complete a new transition',async()=>{
 const {coordinator:c,calls}=fixture({activate:async()=>{throw Error('failed');}});await c.show();const old=c.getSnapshot().owner;await c.detach();
 const current=c.getSnapshot();assert.equal(current.mode,'embedded');assert.ok(current.owner.generation>old.generation);assert.equal(current.error,true);
 c.hostReady(old);assert.equal(c.getSnapshot(),current);assert.deepEqual(calls.filter(x=>x[0]==='claim').map(x=>x[1]),['main','now-playing','main']);
});
test('close during activation rolls back to embedded; normal detached close only closes its view',async()=>{
 const activation=deferred();const {coordinator:c}=fixture({activate:()=>activation.promise});await c.show();const pending=c.detach();await flush();c.detachedClosed();activation.resolve();await pending;
 assert.equal(c.getSnapshot().mode,'embedded');assert.equal(c.getSnapshot().error,true);
 const {coordinator:other}=fixture();await other.show();await other.detach();other.detachedClosed();assert.equal(other.getSnapshot().mode,'closed');
});
test('failure to close after reincorporation rolls back to one detached owner',async()=>{
 const {coordinator:c,calls}=fixture({close:async()=>{throw Error('close failed');}});await c.show();await c.detach();await c.reincorporate();
 assert.equal(c.getSnapshot().mode,'detached');assert.equal(c.getSnapshot().error,true);assert.equal(c.getSnapshot().busy,false);assert.equal(calls.at(-1)[0],'activate');
});
test('envelopes match host/session/generation, not just the target label',()=>{
 const owner={host:'main',sessionId:'fixture',generation:8};assert.equal(analyzerOwnerMatches(owner,{...owner}),true);
 assert.equal(analyzerOwnerMatches({...owner,leaseId:'new'},{...owner,leaseId:'old'}),false);
 assert.equal(analyzerOwnerMatches({...owner,leaseId:'new'},{...owner,leaseId:'new'}),true);
 for(const next of [{...owner,host:'now-playing'},{...owner,generation:7},{...owner,sessionId:'old'}])assert.equal(analyzerOwnerMatches(owner,next),false);
});
test('page presentation restoration preserves mounted nodes, scroll and focus without any load callback',()=>{
 class Element {isConnected=true;scrollTop=0;scrollLeft=0;hidden=false;focus(options){this.focused=options;}closest(){return this.hidden?{}:null;}}
 const previous=globalThis.HTMLElement;globalThis.HTMLElement=Element;
 try{const root=new Element(),child=new Element();child.scrollTop=340;root.scrollLeft=17;root.querySelectorAll=()=>[child];const restore=capturePagePresentation(root,child);child.scrollTop=0;root.scrollLeft=0;restore();assert.equal(child.scrollTop,340);assert.equal(root.scrollLeft,17);assert.deepEqual(child.focused,{preventScroll:true});}
 finally{globalThis.HTMLElement=previous;}
});
