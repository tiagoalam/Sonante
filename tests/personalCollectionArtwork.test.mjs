import test from 'node:test';
import assert from 'node:assert/strict';
import { resolvePersonalCollectionArtwork } from '../src/utils/personalCollectionArtwork.mjs';
import { createCollectionArtworkFallback } from '../src/utils/collectionArtworkFallback.mjs';
const local = {source:'local',album_id:'sqlite:1:a'};
const plex = {source:'plex',server_id:'server',library_id:'library',rating_key:'album'};
const subsonic = {source:'subsonic',account_id:'account',album_id:'album'};
const services = {local:async()=> 'data:image/png;base64,abc', artwork:null};
test('verified artwork metadata needs no additional Plex or Subsonic catalog requests', async () => {
  let calls=0; const read={...services,local:async()=>{ calls++; return 'image'; }};
  assert.equal((await resolvePersonalCollectionArtwork(local,read)).kind,'local');
  const image={server_id:'server',path:'/safe-cover'};
  assert.deepEqual(await resolvePersonalCollectionArtwork(plex,{...read,artwork:{source:'plex',image,session:'session-1'}}),{kind:'plex',image,scope:JSON.stringify(['server','library','session-1'])});
  assert.equal(await resolvePersonalCollectionArtwork({...plex,server_id:'different'},{...read,artwork:{source:'plex',image,session:'session-1'}}),null);
  assert.deepEqual(await resolvePersonalCollectionArtwork(subsonic,{...read,artwork:{source:'subsonic',connection_id:'conn',cover_id:'cover'}}),{kind:'subsonic',connection:'conn',id:'cover'});
  assert.equal(await resolvePersonalCollectionArtwork(null,read),null);
  assert.equal(calls,1);
});
test('fallback shares requests, bounds concurrency, discards obsolete results and retries failures', async () => {
  let active=0, peak=0, calls=0; const finish=[];
  const queue=createCollectionArtworkFallback(async key=>{ calls++; peak=Math.max(peak,++active); await new Promise(resolve=>finish.push(resolve)); active--; if(key==='fail') throw Error('temporary'); return key; });
  let live=true;
  const a=queue('one',()=>live), b=queue('one',()=>true), c=queue('two',()=>true), d=queue('three',()=>true);
  await new Promise(resolve=>setImmediate(resolve)); assert.equal(calls,2); assert.equal(peak,2);
  live=false; finish.shift()(); await new Promise(resolve=>setImmediate(resolve)); assert.equal(calls,3); finish.shift()(); finish.shift()();
  assert.deepEqual(await Promise.all([a,b,c,d]),[null,'one','two','three']);
  const failed=queue('fail',()=>true); await new Promise(resolve=>setImmediate(resolve)); finish.shift()(); await assert.rejects(failed);
  const retried=queue('fail',()=>true); await new Promise(resolve=>setImmediate(resolve)); finish.shift()(); await assert.rejects(retried); assert.equal(calls,5);
});
