import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

import {
  MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS,
  PlexArtworkCache,
  PlexImageVisibilityGate,
  plexCollectionCacheKey,
  plexImageCacheKey,
} from "../src/services/plexArtworkCache.ts";

const nextTurn = () => new Promise((resolve) => setImmediate(resolve));

test("cached leases expose the image synchronously and pin it without another request", async () => {
  let requests = 0;
  const revoked = [];
  const cache = new PlexArtworkCache(async () => { requests++; return new Uint8Array([1]).buffer; }, () => "blob:cached", url => revoked.push(url));
  const image = { server_id: "server-a", path: "/thumb" };
  assert.equal(cache.acquireCached(image), null);
  assert.equal(requests, 0, "a cache miss must not schedule I/O");
  const initial = cache.acquire(image);
  assert.equal(initial.objectUrl, null);
  assert.equal(cache.acquireCached(image), null, "an unfinished request is not a cached image");
  assert.equal(await initial.promise, "blob:cached");
  initial.release();
  const remounted = cache.acquireCached(image);
  assert.equal(remounted.objectUrl, "blob:cached");
  assert.equal(requests, 1);
  cache.clearUnused();
  assert.deepEqual(revoked, []);
  assert.equal(cache.acquireCached({ ...image, server_id: "server-b" }), null);
  remounted.invalidate();
  assert.equal(cache.acquireCached(image), null);
  assert.deepEqual(revoked, ["blob:cached"]);
});

test("PlexImage cached remount bypasses the observer and releases its lease on cleanup", async () => {
  const source = readFileSync(new URL("../src/components/PlexImage.tsx", import.meta.url), "utf8");
  const effect = source.match(/useLayoutEffect\(\(\) => \{([\s\S]*?)\n  \}, \[imageKey\]\);/)?.[1];
  assert.ok(effect);
  let requests = 0, presented;
  const revoked = [];
  const cache = new PlexArtworkCache(async () => { requests++; return new Uint8Array([1]).buffer; }, () => "blob:cached", url => revoked.push(url));
  const image = { server_id: "server-a", path: "/thumb" };
  const first = cache.acquire(image);
  await first.promise;
  first.release();
  const leaseRef = { current: null };
  const run = new Function("artworkCache", "image", "visibilityGateRef", "setObjectUrl", "setFailed", "leaseRef", "containerRef", "window", "IntersectionObserver", "setVisibleImageKey", "imageKey", "PLEX_IMAGE_ROOT_MARGIN", "cacheScope", effect);
  const cleanup = run(cache, image, { current: new PlexImageVisibilityGate() }, url => { presented = url; }, () => {}, leaseRef,
    { current: {} }, {}, () => assert.fail("cached image must not wait for observer"), () => assert.fail("visibility state"), "key", "400px 0px");
  assert.equal(presented, "blob:cached");
  assert.equal(requests, 1);
  cache.clearUnused();
  assert.deepEqual(revoked, []);
  cleanup();
  assert.equal(leaseRef.current, null);
  cache.clearUnused();
  assert.deepEqual(revoked, ["blob:cached"]);
  assert.match(source, /if \(active && visibilityGateRef\.current\.canAcquire\(image, cacheScope\)\) setObjectUrl\(url\)/);
});

test("deduplicates simultaneous image requests and keeps an object URL in use", async () => {
  let requests = 0;
  const revoked = [];
  const cache = new PlexArtworkCache(
    async () => {
      requests += 1;
      await Promise.resolve();
      return new Uint8Array([1, 2, 3]).buffer;
    },
    () => "blob:fixture",
    (url) => revoked.push(url),
    4,
    1024,
  );
  const image = { server_id: "server-a", path: "/library/metadata/1/thumb/1" };

  const first = cache.acquire(image);
  const second = cache.acquire(image);
  assert.equal(await first.promise, "blob:fixture");
  assert.equal(await second.promise, "blob:fixture");
  assert.equal(requests, 1);

  first.release();
  cache.clearUnused();
  assert.deepEqual(revoked, []);
  second.release();
  cache.clearUnused();
  assert.deepEqual(revoked, ["blob:fixture"]);
});

test("PlexImage ignores late success or failure after the visible image changes", async () => {
  const source = readFileSync(new URL("../src/components/PlexImage.tsx", import.meta.url), "utf8");
  const effect = source.match(/  useEffect\(\(\) => \{([\s\S]*?)\n  \}, \[imageKey, visibleImageKey\]\);/)?.[1];
  assert.ok(effect);
  const run = new Function("visibleImageKey", "imageKey", "visibilityGateRef", "image", "artworkCache", "leaseRef", "setObjectUrl", "setFailed", "onLoadErrorRef", "cacheScope", effect);
  for (const outcome of ["success", "failure"]) {
    let finish, presented = null, failed = false, errors = 0;
    const oldImage = { server_id: "server-a", path: "/old" };
    const newImage = { server_id: "server-b", path: "/new" };
    const cache = new PlexArtworkCache(image => image.path === "/old"
      ? new Promise((resolve, reject) => { finish = () => outcome === "success" ? resolve(new Uint8Array([1]).buffer) : reject(new Error("fixture")); })
      : Promise.resolve(new Uint8Array([2]).buffer), bytes => `blob:${new Uint8Array(bytes)[0]}`, () => {});
    const gate = { current: new PlexImageVisibilityGate() }, leaseRef = { current: null };
    const start = image => run(plexImageCacheKey(image), plexImageCacheKey(image), gate, image, cache, leaseRef,
      url => { presented = url; }, value => { failed = value; }, { current: () => { errors++; } });
    gate.current.markVisible(oldImage);
    const oldCleanup = start(oldImage);
    await nextTurn();
    gate.current.reset(newImage);
    gate.current.markVisible(newImage);
    const newCleanup = start(newImage);
    await nextTurn();
    assert.equal(presented, "blob:2");
    finish();
    await nextTurn();
    assert.equal(presented, "blob:2");
    assert.equal(failed, false);
    assert.equal(errors, 0);
    oldCleanup();
    newCleanup();
  }
});

test("collection cache keys include the Plex server identity", () => {
  assert.notEqual(
    plexCollectionCacheKey("server-a", "42"),
    plexCollectionCacheKey("server-b", "42"),
  );
});

test("evicts the least-recently-used object URL after the explicit limit", async () => {
  const revoked = [];
  const cache = new PlexArtworkCache(
    async () => new Uint8Array([1]).buffer,
    (bytes) => `blob:${bytes.byteLength}:${Math.random()}`,
    (url) => revoked.push(url),
    1,
    1024,
  );
  const first = cache.acquire({ server_id: "server-a", path: "/first" });
  const firstUrl = await first.promise;
  first.release();
  const second = cache.acquire({ server_id: "server-a", path: "/second" });
  await second.promise;

  assert.deepEqual(revoked, [firstUrl]);
  second.release();
});

test("limits 100 queued images to eight active requests and starts the next one", async () => {
  let active = 0;
  let maximumActive = 0;
  let started = 0;
  let objectUrls = 0;
  const resolvers = [];
  const cache = new PlexArtworkCache(
    () => {
      started += 1;
      active += 1;
      maximumActive = Math.max(maximumActive, active);
      return new Promise((resolve) => {
        resolvers.push(() => {
          active -= 1;
          resolve(new Uint8Array([1]).buffer);
        });
      });
    },
    () => `blob:${++objectUrls}`,
    () => {},
    128,
    1024,
  );
  const leases = Array.from({ length: 100 }, (_, index) =>
    cache.acquire({ server_id: "server-a", path: `/thumb/${index}` }),
  );

  await nextTurn();
  assert.equal(started, MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS);
  assert.equal(maximumActive, MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS);

  resolvers[0]();
  await nextTurn();
  assert.equal(started, MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS + 1);

  let resolved = 1;
  while (resolved < 100) {
    while (resolved < resolvers.length) {
      resolvers[resolved]();
      resolved += 1;
    }
    await nextTurn();
  }
  await Promise.all(leases.map((lease) => lease.promise));
  assert.equal(maximumActive, MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS);
  leases.forEach((lease) => lease.release());
});

test("a failed request releases its queue slot", async () => {
  let rejectFirst;
  let secondStarted = false;
  let calls = 0;
  const cache = new PlexArtworkCache(
    () => {
      calls += 1;
      if (calls === 1) {
        return new Promise((_, reject) => {
          rejectFirst = reject;
        });
      }
      secondStarted = true;
      return Promise.resolve(new Uint8Array([2]).buffer);
    },
    () => "blob:success",
    () => {},
    4,
    1024,
    1,
  );
  const first = cache.acquire({ server_id: "server-a", path: "/first" });
  const firstResult = first.promise.catch(() => undefined);
  const second = cache.acquire({ server_id: "server-a", path: "/second" });

  await nextTurn();
  assert.equal(secondStarted, false);
  rejectFirst(new Error("fixture failure"));
  await firstResult;
  assert.equal(await second.promise, "blob:success");
  assert.equal(secondStarted, true);
  first.release();
  second.release();
});

test("an invalid Blob URL can be fetched again without revoking shared consumers", async () => {
  let requests = 0;
  const revoked = [];
  const cache = new PlexArtworkCache(
    async () => new Uint8Array([++requests]).buffer,
    (bytes) => `blob:${new Uint8Array(bytes)[0]}`,
    (url) => revoked.push(url),
    4,
    1024,
  );
  const image = { server_id: "server-a", path: "/thumb" };
  const first = cache.acquire(image);
  const shared = cache.acquire(image);
  assert.equal(await first.promise, "blob:1");

  first.invalidate();
  assert.deepEqual(revoked, []);
  const retry = cache.acquire(image);
  assert.equal(await retry.promise, "blob:2");
  assert.equal(requests, 2);

  shared.release();
  assert.deepEqual(revoked, ["blob:1"]);
  retry.release();
  cache.clearUnused();
  assert.deepEqual(revoked, ["blob:1", "blob:2"]);
});

test("visibility gate acquires only after intersection and ignores rerenders", () => {
  const image = { server_id: "server-a", path: "/thumb" };
  const gate = new PlexImageVisibilityGate();
  let acquisitions = 0;

  gate.reset(image);
  if (gate.canAcquire(image)) acquisitions += 1;
  assert.equal(acquisitions, 0);
  if (gate.markVisible(image)) acquisitions += 1;
  assert.equal(acquisitions, 1);
  if (gate.markVisible(image)) acquisitions += 1;
  assert.equal(acquisitions, 1);
});


test("opaque portrait auth scopes share within a context and isolate cached and delayed bytes across accounts", async () => {
  const image={server_id:"server",path:"/library/metadata/artist/thumb/1"},a={},b={};let finish,requests=0;
  const cache=new PlexArtworkCache(()=>{requests++;return requests===1?new Promise(resolve=>{finish=resolve;}):Promise.resolve(new Uint8Array([2]).buffer);},bytes=>`blob:${new Uint8Array(bytes)[0]}`,()=>{});
  const old=cache.acquire(image,a),shared=cache.acquire(image,a);await nextTurn();assert.equal(requests,1);
  assert.equal(cache.acquireCached(image,b),null);
  const next=cache.acquire(image,b);assert.equal(await next.promise,"blob:2");
  finish(new Uint8Array([1]).buffer);assert.equal(await old.promise,"blob:1");assert.equal(await shared.promise,"blob:1");
  const visible=cache.acquireCached(image,b);assert.equal(visible.objectUrl,"blob:2");
  assert.notEqual(plexImageCacheKey(image,a),plexImageCacheKey(image,b));
  const gate=new PlexImageVisibilityGate();gate.markVisible(image,a);assert.equal(gate.canAcquire(image,b),false);assert.equal(gate.markVisible(image,b),true);
  for(const lease of [old,shared,next,visible])lease.release();cache.clearUnused();
});

test("PlexImage drops delayed bytes when only the opaque auth scope changes", async () => {
  const source=readFileSync(new URL("../src/components/PlexImage.tsx",import.meta.url),"utf8");
  const effect=source.match(/  useEffect\(\(\) => \{([\s\S]*?)\n  \}, \[imageKey, visibleImageKey\]\);/)?.[1];assert.ok(effect);
  const run=new Function("visibleImageKey","imageKey","visibilityGateRef","image","artworkCache","leaseRef","setObjectUrl","setFailed","onLoadErrorRef","cacheScope",effect);
  const image={server_id:"server",path:"/library/metadata/artist/thumb/1"},oldScope={},newScope={};let requests=0,finish,presented=null;
  const cache=new PlexArtworkCache(()=>++requests===1?new Promise(resolve=>{finish=resolve;}):Promise.resolve(new Uint8Array([2]).buffer),bytes=>`blob:${new Uint8Array(bytes)[0]}`,()=>{});
  const gate={current:new PlexImageVisibilityGate()},leaseRef={current:null};
  const start=scope=>run(plexImageCacheKey(image,scope),plexImageCacheKey(image,scope),gate,image,cache,leaseRef,url=>{presented=url;},()=>{}, {current:()=>assert.fail("obsolete error")},scope);
  gate.current.markVisible(image,oldScope);const oldCleanup=start(oldScope);await nextTurn();
  gate.current.markVisible(image,newScope);const newCleanup=start(newScope);await nextTurn();assert.equal(presented,"blob:2");
  finish(new Uint8Array([1]).buffer);await nextTurn();assert.equal(presented,"blob:2");oldCleanup();newCleanup();cache.clearUnused();
});
