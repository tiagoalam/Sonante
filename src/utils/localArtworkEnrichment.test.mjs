import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LocalArtworkResolver } from "./localArtwork.ts";
import { LocalArtworkEnrichment } from "./localArtworkEnrichment.ts";

const album = (id) => ({ id, title: `Album ${id}`, artist: "Artist", folder_path: id, discs: [{ folder_path: id }] });
const immediate = () => Promise.resolve();
const deferred = () => {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
async function until(predicate) {
  for (let attempt = 0; attempt < 50; attempt++) {
    if (predicate()) return;
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.fail("enrichment did not reach the expected state");
}

test("three albums run in order with at most one active operation", async () => {
  const gates = [deferred(), deferred(), deferred()];
  const calls = [];
  let active = 0;
  let maxActive = 0;
  const resolver = {
    get: () => null,
    resolve: (item) => {
      calls.push(item.id);
      active++;
      maxActive = Math.max(maxActive, active);
      return gates[calls.length - 1].promise.finally(() => { active--; });
    },
  };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setCatalog([album("A"), album("B"), album("C")]);
  await until(() => calls.length === 1);
  assert.deepEqual(calls, ["A"]);
  gates[0].resolve(null);
  await until(() => calls.length === 2);
  assert.deepEqual(calls, ["A", "B"]);
  gates[1].resolve(null);
  await until(() => calls.length === 3);
  gates[2].resolve(null);
  await until(() => queue.getSnapshot().processed === 3 && !queue.getSnapshot().active);
  assert.equal(maxActive, 1);
  assert.deepEqual(queue.getSnapshot(), { active: false, total: 3, processed: 3, found: 0, latestFoundTitle: undefined });
});

test("known session cover is skipped; a newly found cover is published and counted once", async () => {
  const calls = [];
  const resolver = new LocalArtworkResolver(async (path) => { calls.push(path); return `cover:${path}`; }, async () => null);
  await resolver.resolve(album("A"), false, false);
  const discovered = [];
  resolver.subscribe("B", (cover) => discovered.push(cover));
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setCatalog([album("A"), album("B")]);
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A", "B"]);
  assert.deepEqual(discovered, ["cover:B"]);
  assert.equal(resolver.get("B"), "cover:B");
  assert.equal(queue.getSnapshot().processed, 2);
  assert.equal(queue.getSnapshot().found, 1);
  assert.equal(queue.getSnapshot().latestFoundTitle, "Album B");
  queue.setCatalog([album("A"), album("B")]);
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A", "B"]);
  assert.equal(queue.getSnapshot().found, 1);
});

test("an item error is logged and the next album still runs", async () => {
  const calls = [];
  const errors = [];
  const resolver = {
    get: () => null,
    resolve: async (item) => {
      calls.push(item.id);
      if (item.id === "A") throw new Error("test failure");
      return null;
    },
  };
  const queue = new LocalArtworkEnrichment(resolver, immediate, (error) => errors.push(error));
  queue.setCatalog([album("A"), album("B")]);
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A", "B"]);
  assert.equal(errors.length, 1);
  assert.equal(queue.getSnapshot().processed, 2);
});

test("MPD update pauses before the next item and resumes only with a fresh catalog", async () => {
  const gate = deferred();
  const covers = new Map();
  const calls = [];
  const resolver = {
    get: (id) => covers.get(id) ?? null,
    resolve: (item) => {
      calls.push(item.id);
      return item.id === "A" ? gate.promise.then((cover) => { covers.set(item.id, cover); return cover; }) : Promise.resolve(null);
    },
  };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  const original = [album("A"), album("B")];
  queue.setCatalog(original);
  await until(() => calls.length === 1);
  queue.setUpdating(true);
  gate.resolve("cover:A");
  await until(() => !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A"]);
  assert.equal(queue.getSnapshot().processed, 0);
  queue.setUpdating(false);
  queue.setCatalog(original);
  assert.deepEqual(calls, ["A"]);
  queue.setCatalog([album("A"), album("B")]);
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A", "B"]);
  assert.equal(queue.getSnapshot().processed, 2);
});

test("a fresh catalog arriving before the global update effect is applied still resumes", async () => {
  const calls = [];
  const resolver = { get: () => null, resolve: async (item) => { calls.push(item.id); return null; } };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setCatalog([album("A")]);
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  queue.setUpdating(true);
  queue.setCatalog([album("A"), album("B")]);
  queue.setUpdating(false);
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A", "B"]);
});

test("offline items retry when preference turns on; later items use the current preference", async () => {
  const first = deferred();
  const flags = [];
  let firstCall = true;
  const resolver = {
    get: () => null,
    resolve: async (item, onlineEnabled) => {
      if (item.id === "A" && firstCall) { firstCall = false; await first.promise; }
      flags.push([item.id, onlineEnabled()]);
      return null;
    },
  };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setOnlineEnabled(true);
  queue.setCatalog([album("A"), album("B")]);
  await until(() => queue.getSnapshot().active);
  queue.setOnlineEnabled(false);
  first.resolve();
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(flags, [["A", false], ["B", false]]);
  queue.setOnlineEnabled(true);
  await until(() => flags.length === 4 && !queue.getSnapshot().active);
  assert.deepEqual(flags, [["A", false], ["B", false], ["A", true], ["B", true]]);
  assert.equal(queue.getSnapshot().processed, 2);
});

test("a preference enabled during an unfinished offline item retries that item", async () => {
  const gate = deferred();
  const calls = [];
  const resolver = {
    get: () => null,
    resolve: async (item, enabled) => {
      calls.push(enabled());
      if (calls.length === 1) await gate.promise;
      return null;
    },
  };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setCatalog([album("A")]);
  await until(() => calls.length === 1);
  queue.setOnlineEnabled(true);
  gate.resolve();
  await until(() => calls.length === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, [false, true]);
  assert.equal(queue.getSnapshot().processed, 1);
});

test("new catalog keeps existing IDs processed and adds only new IDs", async () => {
  const calls = [];
  const resolver = { get: () => null, resolve: async (item) => { calls.push(item.id); return null; } };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setOnlineEnabled(true);
  queue.setCatalog([album("A")]);
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  queue.setCatalog([album("A"), album("B")]);
  await until(() => queue.getSnapshot().processed === 2 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["A", "B"]);
  assert.equal(queue.getSnapshot().total, 2);
  assert.equal(queue.getSnapshot().processed, 2);
  queue.setCatalog([album("B")]);
  await until(() => queue.getSnapshot().total === 1);
  assert.equal(queue.getSnapshot().total, 1);
  assert.equal(queue.getSnapshot().processed, 1);
});

test("a stale generation cannot publish its old progress", async () => {
  const old = deferred();
  const next = deferred();
  const calls = [];
  const resolver = { get: () => null, resolve: (item) => { calls.push(item.id); return item.id === "A" ? old.promise : next.promise; } };
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setCatalog([album("A")]);
  await until(() => calls.length === 1);
  queue.setCatalog([album("B")]);
  old.resolve("old cover");
  await until(() => calls.length === 2);
  assert.equal(queue.getSnapshot().total, 1);
  assert.equal(queue.getSnapshot().processed, 0);
  assert.equal(queue.getSnapshot().found, 0);
  next.resolve("new cover");
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  assert.equal(queue.getSnapshot().processed, 1);
  assert.equal(queue.getSnapshot().found, 1);
});

test("lazy card and background share one in-flight resolution", async () => {
  const gate = deferred();
  let localCalls = 0;
  let onlineCalls = 0;
  const resolver = new LocalArtworkResolver(async () => { localCalls++; return gate.promise; }, async () => { onlineCalls++; return null; });
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setOnlineEnabled(true);
  queue.setCatalog([album("A")]);
  await until(() => localCalls === 1);
  const lazy = resolver.resolve(album("A"), true, false);
  assert.equal(localCalls, 1);
  gate.resolve("local cover");
  assert.equal(await lazy, "local cover");
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  assert.equal(onlineCalls, 0);
  assert.equal(queue.getSnapshot().found, 1);
});

test("an older online result cannot replace a cover published during its request", async () => {
  const online = deferred();
  let localCalls = 0;
  let onlineCalls = 0;
  const resolver = new LocalArtworkResolver(async () => ++localCalls === 1 ? null : "local cover", async () => { onlineCalls++; return online.promise; });
  const full = resolver.resolve(album("A"), true, false);
  await until(() => onlineCalls === 1);
  assert.equal(await resolver.resolveLocalOnly(album("A")), "local cover");
  online.resolve("online cover");
  assert.equal(await full, "local cover");
  assert.equal(resolver.get("A"), "local cover");
});

test("disabled online lookup stays offline and missing albums retry when enabled", async () => {
  let onlineCalls = 0;
  const resolver = new LocalArtworkResolver(async () => null, async () => { onlineCalls++; return "online cover"; });
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setCatalog([album("A")]);
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  assert.equal(onlineCalls, 0);
  assert.equal(queue.getSnapshot().processed, 1);
  queue.setOnlineEnabled(true);
  await until(() => onlineCalls === 1 && !queue.getSnapshot().active);
  assert.equal(onlineCalls, 1);
  assert.equal(queue.getSnapshot().found, 1);
});

test("four disc folders keep their existing order before the online phase", async () => {
  const paths = [];
  let onlineCalls = 0;
  const resolver = new LocalArtworkResolver(async (path) => {
    paths.push(path);
    return path === "Album/CD4" ? "embedded cover" : null;
  }, async () => { onlineCalls++; return "online cover"; });
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setOnlineEnabled(true);
  queue.setCatalog([{
    ...album("A"), folder_path: "Album/CD1",
    discs: [1, 2, 3, 4].map((n) => ({ folder_path: `Album/CD${n}` })),
  }]);
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  assert.deepEqual(paths, ["Album/CD1", "Album/CD2", "Album/CD3", "Album/CD4"]);
  assert.equal(onlineCalls, 0);
  assert.equal(queue.getSnapshot().found, 1);
});

test("status strings exist in both locales", () => {
  for (const locale of ["en", "pt-BR"]) {
    const artwork = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8")).artwork;
    assert.ok(artwork.searchingOnline);
    assert.ok(artwork.foundOnline);
  }
});

test("persisted local, embedded, online and valid negative skip the global queue on restart", async () => {
  const entries = Object.fromEntries(["local", "embedded", "online", "negative"].map((id) => [id, {
    result: id === "negative" ? "none" : id,
    online_complete: id === "online" || id === "negative",
    checked_at: 100,
  }]));
  const calls = [];
  const store = { load: async () => entries, save: async () => {} };
  const queue = new LocalArtworkEnrichment({ get: () => null, resolve: async (item) => { calls.push(item.id); return null; } }, immediate, console.error, store);
  queue.setOnlineEnabled(true);
  queue.setCatalog(["local", "embedded", "online", "negative", "new"].map(album));
  await until(() => queue.getSnapshot().processed === 5 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["new"]);
});

test("a cover found first by a lazy card keeps its online cache origin", async () => {
  const writes = [];
  const resolver = new LocalArtworkResolver(async () => null, async () => "online cover", 150, async () => "positive");
  await resolver.resolve(album("A"), true, false);
  const store = { load: async () => ({}), save: async (changes) => { writes.push(...changes); } };
  const queue = new LocalArtworkEnrichment(resolver, immediate, console.error, store);
  queue.setCatalog([album("A")]);
  await until(() => writes.length === 1);
  assert.deepEqual(writes[0], { album_id: "A", result: "online", online_complete: true });
});

test("offline progress remains eligible when preference is enabled later", async () => {
  const entries = { A: { result: "none", online_complete: false, checked_at: 100 } };
  const calls = [];
  const store = { load: async () => entries, save: async (changes) => { for (const change of changes) entries[change.album_id] = { result: change.result, online_complete: change.online_complete, checked_at: 101 }; } };
  const queue = new LocalArtworkEnrichment({ get: () => null, resolve: async (item, enabled) => { calls.push([item.id, enabled()]); return null; } }, immediate, console.error, store);
  queue.setCatalog([album("A")]);
  await until(() => queue.getSnapshot().processed === 1);
  assert.deepEqual(calls, []);
  queue.setOnlineEnabled(true);
  await until(() => calls.length === 1 && !queue.getSnapshot().active);
  assert.deepEqual(calls, [["A", true]]);
});

test("progress writes are batched instead of one write per album", async () => {
  const writes = [];
  const store = { load: async () => ({}), save: async (changes) => { writes.push(changes); } };
  const queue = new LocalArtworkEnrichment({ get: () => null, resolve: async () => null }, immediate, console.error, store);
  queue.setCatalog(Array.from({ length: 201 }, (_, n) => album(String(n))));
  await until(() => queue.getSnapshot().processed === 201 && !queue.getSnapshot().active);
  await until(() => writes.flat().length === 201);
  assert.equal(writes.length, 2);
});

test("local and cache hits are silent; real online request and success are shown", async () => {
  const gate = deferred();
  const resolver = new LocalArtworkResolver(
    async (path) => path === "local" ? "local cover" : null,
    async () => gate.promise,
    150,
    async (item) => item.id === "negative" ? "negative" : "miss",
  );
  const queue = new LocalArtworkEnrichment(resolver, immediate);
  queue.setOnlineEnabled(true);
  queue.setCatalog([album("local"), album("negative"), album("online")]);
  await until(() => queue.getSnapshot().onlineActivity?.kind === "searching");
  assert.equal(queue.getSnapshot().onlineActivity.title, "Album online");
  gate.resolve("online cover");
  await until(() => queue.getSnapshot().onlineActivity?.kind === "found");
  assert.equal(queue.getSnapshot().onlineActivity.title, "Album online");
});

test("online no-match and temporary errors leave no user-facing notification", async () => {
  const noMatch = new LocalArtworkResolver(async () => null, async () => null, 150, async () => "miss");
  const queue = new LocalArtworkEnrichment(noMatch, immediate);
  queue.setOnlineEnabled(true);
  queue.setCatalog([album("A")]);
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  assert.equal(queue.getSnapshot().onlineActivity, undefined);

  const errors = [];
  const failing = new LocalArtworkResolver(async () => null, async () => { throw new Error("temporary"); }, 150, async () => "miss");
  const failedQueue = new LocalArtworkEnrichment(failing, immediate, (error) => errors.push(error));
  failedQueue.setOnlineEnabled(true);
  failedQueue.setCatalog([album("B")]);
  await until(() => failedQueue.getSnapshot().processed === 1 && !failedQueue.getSnapshot().active);
  assert.equal(failedQueue.getSnapshot().onlineActivity, undefined);
  assert.equal(errors.length, 1);
});

test("catalog reconciliation waits for browser yield and stale generation stays safe", async () => {
  const paint = deferred();
  const calls = [];
  const queue = new LocalArtworkEnrichment({ get: () => null, resolve: async (item) => { calls.push(item.id); return null; } }, () => paint.promise);
  queue.setCatalog([album("old")]);
  queue.setCatalog([album("new")]);
  assert.deepEqual(calls, []);
  paint.resolve();
  await until(() => queue.getSnapshot().processed === 1 && !queue.getSnapshot().active);
  assert.deepEqual(calls, ["new"]);
});
