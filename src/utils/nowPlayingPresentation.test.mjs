import test from 'node:test';
import assert from 'node:assert/strict';
import { QueuePreviewReader, queueItemIsCurrent, queueVisibleRange, nowPlayingWindowBounds } from './nowPlayingPresentation.ts';
const flush=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
const local={title:'Same name',artist:'Same artist',uri:'first.flac',media_locator:{kind:'local',uri:'first.flac'}};
const plex={...local,uri:undefined,media_locator:{kind:'plex',server_id:'server-one',part_key:'/library/parts/1/file'}};

test('queue identity includes position, local locator and Plex server/part; duplicates remain distinct',()=>{
 assert.equal(queueItemIsCurrent(local,0,{kind:'local',uri:'first.flac',queue_index:0}),true);
 assert.equal(queueItemIsCurrent({...local,uri:'legacy-wrong'},0,{kind:'local',uri:'first.flac',queue_index:0}),true);
 assert.equal(queueItemIsCurrent(local,1,{kind:'local',uri:'first.flac',queue_index:0}),false);
 assert.equal(queueItemIsCurrent({...local,media_locator:{kind:'local',uri:'other.flac'}},0,{kind:'local',uri:'first.flac',queue_index:0}),false);
 assert.equal(queueItemIsCurrent(plex,1,{kind:'plex',server_id:'server-one',part_key:'/library/parts/1/file',queue_index:1}),true);
 assert.equal(queueItemIsCurrent(plex,1,{kind:'plex',server_id:'server-two',part_key:'/library/parts/1/file',queue_index:1}),false);
 assert.equal(queueItemIsCurrent(plex,1,{kind:'plex',server_id:'server-one',part_key:'/library/parts/2/file',queue_index:1}),false);
 assert.equal(queueItemIsCurrent(local,0,null),false);
});
test('reader is read only, preserves order/duplicates, ignores a closed result and serializes rapid reopen',async()=>{
 let calls=0;const first=deferred(),second=deferred(),states=[];
 const reader=new QueuePreviewReader(()=>{calls++;return calls===1?first.promise:second.promise;},s=>states.push(s),10000);
 reader.setActive(true);reader.setActive(true);assert.equal(calls,1);
 reader.setActive(false);reader.setActive(true);assert.equal(calls,1);
 first.resolve([plex]);await flush();assert.equal(calls,2);assert.ok(!states.some(s=>s.items.length));
 second.resolve([local,plex,local]);await flush();assert.deepEqual(states.at(-1),{phase:'ready',items:[local,plex,local]});reader.setActive(false);
});
test('reader handles empty/failure and continues updates, with no reads while hidden',async()=>{
 let calls=0;const states=[];
 const reader=new QueuePreviewReader(async()=>{calls++;if(calls===2)throw Error('private details');return calls===1?[]:[plex];},s=>states.push(s),5);
 assert.equal(calls,0);reader.setActive(true);await flush();assert.equal(states.at(-1).phase,'ready');assert.deepEqual(states.at(-1).items,[]);
 await new Promise(r=>setTimeout(r,8));assert.equal(states.at(-1).phase,'error');assert.doesNotMatch(JSON.stringify(states),/private/);
 await new Promise(r=>setTimeout(r,8));reader.setActive(false);assert.ok(states.some(s=>s.items[0]===plex));const last=calls;
 await new Promise(r=>setTimeout(r,12));assert.equal(calls,last);
});
test('unavailable/closed queue discards failures as well as stale success',async()=>{
 const pending=deferred(),states=[];const reader=new QueuePreviewReader(()=>pending.promise,s=>states.push(s));
 reader.setActive(true);reader.setActive(false);pending.reject(Error('offline'));await flush();assert.deepEqual(states.map(s=>s.phase),['loading']);
});
test('virtual queue limits artwork rows even for a large queue, and reaches the tail',()=>{
 const first=queueVisibleRange(100000,0,480);assert.deepEqual(first,{start:0,end:16});
 const middle=queueVisibleRange(100000,64000,480);assert.equal(middle.start,996);assert.equal(middle.end-middle.start,16);
 const tail=queueVisibleRange(100,99*64,480);assert.equal(tail.end,100);
 assert.deepEqual(queueVisibleRange(0,0,480),{start:0,end:0});
});
test('work-area sizing uses one logical coordinate system with margins at all scales and monitor origins',()=>{
 for(const scale of [1,1.25,1.5,2])for(const area of [
  {position:{x:0,y:32},size:{width:1920,height:1000}},
  {position:{x:-2560,y:-200},size:{width:2560,height:1400}},
  {position:{x:100,y:50},size:{width:640,height:480}}
 ]){
  const b=nowPlayingWindowBounds(area,scale);
  assert.ok(b.width<=area.size.width/scale&&b.height<=area.size.height/scale);
  assert.ok(b.minWidth<=b.width&&b.minHeight<=b.height);
  assert.ok(b.x>=area.position.x/scale-1&&b.y>=area.position.y/scale-1);
  assert.ok(b.x+b.width<=(area.position.x+area.size.width)/scale);
  assert.ok(b.y+b.height<=(area.position.y+area.size.height)/scale);
 }
 assert.throws(()=>nowPlayingWindowBounds({position:{x:0,y:0},size:{width:1920,height:1000}},0));
});

test('explicit play revalidates the complete positional queue and never matches by title',async()=>{
 const {playConfirmedQueuePosition}=await import('./nowPlayingPresentation.ts');const calls=[];
 await playConfirmedQueuePosition([local,plex,local],2,async()=>[local,plex,local],async i=>calls.push(i),()=>true);assert.deepEqual(calls,[2]);
 await assert.rejects(playConfirmedQueuePosition([local,plex,local],2,async()=>[plex,local,local],async()=>assert.fail(),()=>true));
 await assert.rejects(playConfirmedQueuePosition([local],0,async()=>[{...local,media_locator:{kind:'local',uri:'different.flac'}}],async()=>assert.fail(),()=>true));
 let available=true;await assert.rejects(playConfirmedQueuePosition([local],0,async()=>{available=false;return[local];},async()=>assert.fail(),()=>available));
});
test('refresh invalidates an in-flight queue read after successful clear',async()=>{
 const pending=deferred(),states=[];let calls=0;const reader=new QueuePreviewReader(()=>++calls===1?pending.promise:Promise.resolve([]),s=>states.push(s),10000);
 reader.setActive(true);reader.refresh([]);pending.resolve([local]);await flush();assert.equal(calls,2);assert.deepEqual(states.at(-1).items,[]);assert.equal(states.at(-1).phase,'ready');reader.setActive(false);
});
