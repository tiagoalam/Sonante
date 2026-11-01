import test from 'node:test';
import assert from 'node:assert/strict';
import { VisibleNavidromeQueueArtwork } from './visibleNavidromeQueueArtwork.ts';
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve}};
const flush=async()=>{for(let i=0;i<30;i++)await Promise.resolve()};
const track=(song,connection='session')=>({title:'Same title',media_locator:{kind:'navidrome',connection_id:connection,song_id:song}});
test('visible Navidrome artwork has two slots, deduplicates exact identities and never retries misses on polling',async()=>{
 const calls=[],pending=[];let changed=0;const worker=new VisibleNavidromeQueueArtwork(async(connection,song)=>{calls.push([connection,song]);const wait=deferred();pending.push(wait);return wait.promise},()=>changed++);
 const visit={};worker.begin(visit);const rows=[track('a'),track('a'),track('b'),track('c')];worker.show(rows);await flush();assert.equal(calls.length,2);
 pending[0].resolve('cover-a');await flush();assert.equal(calls.length,3);pending[1].resolve(null);pending[2].resolve('cover-c');await flush();
 worker.show(rows.map(row=>({...row})));await flush();assert.equal(calls.length,3);assert.equal(changed,2);assert.equal(worker.get(visit,rows[0]),'cover-a');assert.equal(worker.get(visit,rows[2]),null);
 worker.show([track('a','other')]);await flush();pending[3].resolve('other-cover');await flush();assert.equal(worker.get(visit,track('a','other')),'other-cover');assert.equal(worker.get(visit,rows[0]),null);
});
test('old in-flight requests keep their slots while a new visit waits, and stale results cannot publish across sessions',async()=>{
 const pending=[],calls=[];let changed=0;const worker=new VisibleNavidromeQueueArtwork(async(_connection,song)=>{calls.push(song);const wait=deferred();pending.push(wait);return wait.promise},()=>changed++);
 worker.begin({});worker.show([track('old-a'),track('old-b'),track('never-start')]);await flush();const visit={};worker.begin(visit);worker.show([track('new')]);await flush();assert.deepEqual(calls,['old-a','old-b']);
 pending[0].resolve('old-cover');await flush();assert.deepEqual(calls,['old-a','old-b','new']);pending[1].resolve('old-cover');pending[2].resolve('new-cover');await flush();assert.equal(changed,1);assert.equal(worker.get(visit,track('new')),'new-cover');
 worker.begin(null);assert.equal(worker.get(visit,track('new')),null);
});
test('closed, other-source, already-covered and pre-dispatch obsolete rows perform no reads',async()=>{
 let calls=0;const worker=new VisibleNavidromeQueueArtwork(async()=>{calls++;return 'cover'},()=>assert.fail('no stale publish'));
 worker.show([track('closed')]);await flush();assert.equal(calls,0);
 worker.begin({});worker.show([{media_locator:{kind:'local',uri:'a.flac'}},{media_locator:{kind:'plex',server_id:'s',part_key:'p'}},{...track('covered'),thumb:'current'}]);await flush();assert.equal(calls,0);
 worker.show([track('cancelled')]);worker.begin(null);await flush();assert.equal(calls,0);
});
