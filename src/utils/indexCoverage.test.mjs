import test from'node:test';import assert from'node:assert/strict';import{createIndexCoverageStore,coverageSnapshot}from'./indexCoverage.ts';
const flush=async()=>{for(let i=0;i<20;i++)await Promise.resolve()};
const report=(omitted=1)=>({outcome:omitted?'partial':'confirmed',generation:'fixture-1',scope:'full',sources:[{source:'Music',audio_files:2,cue_groups:0,cataloged_audio:2-omitted,virtual_tracks:0,omitted,policy_excluded:0}]});
const snapshot=(revision=1,omitted=1)=>({revision,report:report(omitted)});
test('one subscription/query serves mounted consumers and stale queries cannot erase a partial report',async()=>{
 let resolveRead,receive,reads=0,listens=0,stopped=0;const store=createIndexCoverageStore(()=>{reads++;return new Promise(r=>resolveRead=r)},async fn=>{listens++;receive=fn;return()=>stopped++});const a=store.subscribe(()=>{}),b=store.subscribe(()=>{});await flush();receive(snapshot(3));resolveRead({revision:2,report:null});await flush();assert.equal(store.getSnapshot().report.outcome,'partial');receive({revision:3,report:null});assert.equal(store.getSnapshot().report.outcome,'partial');receive(snapshot(1,0));assert.equal(store.getSnapshot().revision,3);assert.equal(reads,1);assert.equal(listens,1);a();assert.equal(stopped,0);b();assert.equal(stopped,1);
});
test('new scope may invalidate report; equal-revision confirmed event fills an unknown query without rollback',async()=>{
 let receive;const store=createIndexCoverageStore(async()=>({revision:0,report:null}),async fn=>{receive=fn;return()=>{}});const close=store.subscribe(()=>{});await flush();receive(snapshot(1));receive({revision:2,report:null});assert.equal(store.getSnapshot().report,null);receive(snapshot(2,0));assert.equal(store.getSnapshot().report.outcome,'confirmed');receive(snapshot(1));assert.equal(store.getSnapshot().revision,2);close();
});
test('cleanup rejects late listeners/read results and restarting has one live subscription',async()=>{
 let resolveListen,resolveRead,receive,stopped=0;const store=createIndexCoverageStore(()=>new Promise(r=>resolveRead=r),fn=>{receive=fn;return new Promise(r=>resolveListen=r)});const first=store.subscribe(()=>{});first();resolveListen(()=>stopped++);await flush();assert.equal(stopped,1);assert.equal(resolveRead,undefined);const second=store.subscribe(()=>{});resolveListen(()=>stopped++);await flush();second();receive(snapshot(99));resolveRead(snapshot(99));await flush();assert.equal(store.getSnapshot().report,null);assert.equal(stopped,2);
});
test('invalid payloads never become a success or expose raw paths',()=>{
 for(const bad of [{revision:-1,report:null},{revision:1,report:{}},{...snapshot(),report:{...report(),outcome:'success'}},{...snapshot(),report:{...report(),sources:[{...report().sources[0],source:'/private/path'}]}},{...snapshot(),report:{...report(),sources:[{...report().sources[0],omitted:-1}]}}])assert.equal(coverageSnapshot(bad),null);
});
