import test from 'node:test';
import assert from 'node:assert/strict';
import {NavidromeAlbumFeed} from '../src/utils/navidromeAlbumFeed.ts';
const album=id=>({id,title:id,artist:'Fixture',year:null,cover_id:null,song_count:null});
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
test('incremental catalog deduplicates overlapping batches but advances by raw rows; duplicate triggers share one request',async()=>{
 const pending=deferred(),states=[],offsets=[];
 const feed=new NavidromeAlbumFeed(async offset=>{offsets.push(offset);return offset===0?pending.promise:{albums:[album('b'),album('c')],offset,more:false};},state=>states.push(state));
 const first=feed.loadMore();await feed.loadMore();assert.deepEqual(offsets,[0]);pending.resolve({albums:[album('a'),album('a'),album('b')],offset:0,more:true});await first;
 await feed.loadMore();await feed.loadMore();assert.deepEqual(offsets,[0,3]);assert.deepEqual(states.at(-1).albums.map(a=>a.id),['a','b','c']);assert.equal(states.at(-1).more,false);
});
test('retired connection or query cannot publish a late success or failure',async()=>{
 for(const fail of [false,true]){const pending=deferred(),states=[];const feed=new NavidromeAlbumFeed(()=>pending.promise,state=>states.push(state));const task=feed.loadMore();feed.dispose();if(fail)pending.reject('PRIVATE');else pending.resolve({albums:[album('stale')],offset:0,more:false});await task;assert.equal(states.length,1);await feed.loadMore();assert.equal(states.length,1);}
});
test('an append failure preserves loaded objects and retries the same offset without losing progress',async()=>{
 let attempt=0;const states=[],offsets=[];const feed=new NavidromeAlbumFeed(async offset=>{offsets.push(offset);if(!offset)return{albums:[album('a')],offset,more:true};if(++attempt===1)throw 'NAVIDROME_UNAVAILABLE';return{albums:[album('b')],offset,more:false}},state=>states.push(state));
 await feed.loadMore();const kept=states.at(-1).albums[0];await feed.loadMore();assert.equal(states.at(-1).albums[0],kept);assert.equal(states.at(-1).error,'NAVIDROME_UNAVAILABLE');await feed.loadMore();assert.deepEqual(offsets,[0,1,1]);assert.deepEqual(states.at(-1).albums.map(a=>a.id),['a','b']);
});
test('repeated non-progress pages and wrong offsets stop with retryable errors; an empty end never loops',async()=>{
 for(const mode of ['repeated','offset','empty']){const states=[];let reads=0;const feed=new NavidromeAlbumFeed(async offset=>{reads++;return offset===0?{albums:[album('a')],offset,more:true}:mode==='empty'?{albums:[],offset,more:true}:{albums:[album('a')],offset:mode==='offset'?0:offset,more:true}},state=>states.push(state));await feed.loadMore();await feed.loadMore();assert.equal(reads,2);assert.deepEqual(states.at(-1).albums.map(a=>a.id),['a']);if(mode==='empty'){assert.equal(states.at(-1).more,false);await feed.loadMore();assert.equal(reads,2)}else assert.equal(states.at(-1).error,'NAVIDROME_PROTOCOL');}
});


test('failed explicit refresh retains completed inventory and total; retry restarts at zero and replaces only after success',async()=>{
 const states=[],offsets=[],pending=deferred();let reads=0;
 const feed=new NavidromeAlbumFeed(async offset=>{offsets.push(offset);reads++;if(reads===1)return{albums:[album('known')],offset,more:false,total:1};if(reads===2)throw 'NAVIDROME_UNAVAILABLE';return pending.promise;},state=>states.push(state));
 await feed.loadMore();const known=states.at(-1).albums;await feed.refresh();assert.equal(states.at(-1).albums,known);assert.equal(states.at(-1).total,1);assert.equal(states.at(-1).error,'NAVIDROME_UNAVAILABLE');
 const retry=feed.loadMore();await feed.loadMore();assert.equal(states.at(-1).albums,known);assert.equal(states.at(-1).loading,true);assert.deepEqual(offsets,[0,0,0]);pending.resolve({albums:[album('new')],offset:0,more:false,total:1});await retry;assert.deepEqual(states.at(-1).albums.map(a=>a.id),['new']);assert.equal(states.at(-1).error,null);
});
test('refresh with unknown server total does not reuse an obsolete count; retiring an update preserves its old published inventory',async()=>{
 const states=[];let reads=0;const pending=deferred();const feed=new NavidromeAlbumFeed(async offset=>++reads===1?{albums:[album('a')],offset,more:false,total:30}:reads===2?{albums:[album('b')],offset,more:true}:pending.promise,state=>states.push(state));
 await feed.loadMore();await feed.refresh();assert.equal(states.at(-1).total,null);assert.deepEqual(states.at(-1).albums.map(a=>a.id),['b']);const update=feed.refresh();feed.dispose();const count=states.length;pending.resolve({albums:[album('stale')],offset:0,more:false,total:1});await update;assert.equal(states.length,count);
});
