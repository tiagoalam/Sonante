import test from "node:test";
import assert from "node:assert/strict";
import { PlexArtistPortraitCache, ARTIST_PORTRAIT_RETRY_MS, artistPortraitInitials } from "./plexArtistPortraits.ts";
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<24;i++)await Promise.resolve();};
const portrait=(server,key)=>({server_id:server,path:`/library/metadata/${key}/thumb/1`});

test("exact server/key shares queries across library cards; absent keys and Local never become nominal lookups", async()=>{
  const calls=[],identity={},cache=new PlexArtistPortraitCache(async(s,k)=>{calls.push([s,k]);return portrait(s,k);});cache.setContext(identity,"server");
  const a=cache.acquire(identity,"server","one"),b=cache.acquire(identity,"server","one");
  assert.equal(a.promise,b.promise);assert.equal((await a.promise).status,"ready");assert.deepEqual(calls,[["server","one"]]);
  a.release();b.release();const visit=cache.acquire(identity,"server","one");await visit.promise;visit.release();assert.equal(calls.length,1);
  for(const key of ["","name:Same name","one/album"]){const lease=cache.acquire(identity,"server",key);assert.equal((await lease.promise).status,"obsolete");}
  const wrong=cache.acquire(identity,"other","one");assert.equal((await wrong.promise).status,"obsolete");assert.equal(calls.length,1);
  const otherIdentity={};cache.setContext(otherIdentity,"other");const another=cache.acquire(otherIdentity,"other","one");await another.promise;another.release();assert.deepEqual(calls.at(-1),["other","one"]);
});

test("two metadata queries maximum, cancellation skips invisible queued artists, and cache is bounded", async()=>{
  const calls=[],identity={},cache=new PlexArtistPortraitCache((s,k)=>{const d=deferred();calls.push({s,k,...d});return d.promise;},Date.now,3);cache.setContext(identity,"server");
  const a=cache.acquire(identity,"server","a"),b=cache.acquire(identity,"server","b"),c=cache.acquire(identity,"server","c");await flush();assert.equal(calls.length,2);
  c.release();assert.equal((await c.promise).status,"obsolete");
  const d=cache.acquire(identity,"server","d");await flush();assert.equal(calls.length,2);
  calls[0].resolve(portrait("server","a"));await a.promise;a.release();await flush();assert.equal(calls.length,3);assert.equal(calls[2].k,"d");
  calls[1].resolve(null);await b.promise;b.release();calls[2].resolve(null);await d.promise;d.release();
  const e=cache.acquire(identity,"server","e");await flush();assert.equal(calls.length,4);assert.equal(cache.entries.size,3);assert.equal(cache.peek(identity,"server","a"),null);
  calls[3].resolve(null);await e.promise;e.release();
});

test("absence remains distinct from temporary failure; retries occur only on later acquisition after cooldown", async()=>{
  let now=0,failures=0,misses=0;const identity={};
  const cache=new PlexArtistPortraitCache(async(s,k)=>{if(k==="missing"){misses++;return null;}failures++;if(failures===1)throw Error("SYNTHETIC PRIVATE ERROR");return portrait(s,k);},()=>now);cache.setContext(identity,"server");
  let lease=cache.acquire(identity,"server","missing");assert.deepEqual(await lease.promise,{status:"missing"});lease.release();
  lease=cache.acquire(identity,"server","failed");assert.deepEqual(await lease.promise,{status:"error"});lease.release();
  for(let i=0;i<10;i++){lease=cache.acquire(identity,"server","failed");assert.deepEqual(await lease.promise,{status:"error"});lease.release();}
  assert.equal(failures,1);now+=ARTIST_PORTRAIT_RETRY_MS;
  assert.equal(cache.peek(identity,"server","failed").status,"error", "reading alone never retries");
  lease=cache.acquire(identity,"server","failed");assert.equal((await lease.promise).status,"ready");lease.release();assert.equal(failures,2);
  lease=cache.acquire(identity,"server","missing");assert.equal((await lease.promise).status,"missing");lease.release();assert.equal(misses,1);
});

test("logout/account/server invalidation discards old replies and preserves the two-request budget across contexts", async()=>{
  for(const change of ["logout","account","server"]){
    const calls=[],identity={},cache=new PlexArtistPortraitCache((s,k)=>{const d=deferred();calls.push({s,k,...d});return d.promise;});cache.setContext(identity,"server");
    const a=cache.acquire(identity,"server","a"),b=cache.acquire(identity,"server","b"),queued=cache.acquire(identity,"server","queued");await flush();
    const next={};if(change==="logout")cache.invalidate();else cache.setContext(next,change==="server"?"other":"server");
    assert.equal((await a.promise).status,"obsolete");assert.equal((await queued.promise).status,"obsolete");
    const server=change==="server"?"other":"server";const fresh=cache.acquire(next,server,"fresh");await flush();assert.equal(calls.length,2);
    calls[0].resolve(portrait("server","a"));calls[1].reject(Error("old"));await flush();
    assert.equal(cache.peek(identity,"server","a"),null);assert.ok(!calls.some(c=>c.k==="queued"));
    if(change!=="logout"){assert.equal(calls.length,3);calls[2].resolve(portrait(server,"fresh"));assert.equal((await fresh.promise).status,"ready");}
    else assert.equal((await fresh.promise).status,"obsolete");
    a.release();b.release();queued.release();fresh.release();cache.invalidate();
  }
});

test("wrong-server metadata is a failure, and initials support Unicode without a photo dependency",async()=>{
  const identity={},cache=new PlexArtistPortraitCache(async()=>portrait("other","one"));cache.setContext(identity,"server");
  const lease=cache.acquire(identity,"server","one");assert.deepEqual(await lease.promise,{status:"error"});lease.release();
  assert.equal(artistPortraitInitials("  Nina   Simone "),"NS");assert.equal(artistPortraitInitials("BJO\u0308RK"),"B");
  assert.equal(artistPortraitInitials("宇多田 ヒカル"),"宇ヒ");assert.equal(artistPortraitInitials("???"),null);
});
