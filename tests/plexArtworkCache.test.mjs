import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS,
  PlexArtworkCache,
  PlexImageVisibilityGate,
  plexCollectionCacheKey,
} from "../src/services/plexArtworkCache.ts";

const nextTurn = () => new Promise((resolve) => setImmediate(resolve));

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
