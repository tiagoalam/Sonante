import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { lookupAlbumArtwork, LocalArtworkResolver, artworkReadBlocked } from "./localArtwork.ts";

const album = {
  id: "root|album", title: "Album", artist: "Artist", year: "2000",
  folder_path: "root/CD1", discs: [
    { folder_path: "root/CD1" },
    { folder_path: "root/CD2" },
    { folder_path: "root/CD3" },
  ],
};

test("local artwork wins and online is not called", async () => {
  let online = 0;
  const result = await lookupAlbumArtwork(album, async () => "local", async () => { online++; return "online"; }, true, false);
  assert.equal(result, "local");
  assert.equal(online, 0);
});

test("online waits for all discs and is skipped when disabled or updating", async () => {
  const paths = [];
  let online = 0;
  const local = async (path) => { paths.push(path); return null; };
  const remote = async () => { online++; return "online"; };
  assert.equal(await lookupAlbumArtwork(album, local, remote, false, false), null);
  paths.length = 0;
  assert.equal(await lookupAlbumArtwork(album, local, remote, true, true), null);
  assert.deepEqual(paths, [], "updating must skip local I/O too");
  assert.equal(online, 0);
  paths.length = 0;
  assert.equal(await lookupAlbumArtwork(album, local, remote, true, false), "online");
  assert.deepEqual(paths, ["root/CD1", "root/CD2", "root/CD3"]);
  assert.equal(online, 1);
});

test("simultaneous online lookup for the same album is deduplicated", async () => {
  let calls = 0;
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const remote = async () => { calls++; return pending; };
  const first = lookupAlbumArtwork(album, async () => null, remote, true, false);
  const second = lookupAlbumArtwork(album, async () => null, remote, true, false);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls, 1);
  release("cover");
  assert.deepEqual(await Promise.all([first, second]), ["cover", "cover"]);
});

test("an update beginning during local lookup prevents a new online request", async () => {
  let updating = false;
  let online = 0;
  const result = await lookupAlbumArtwork(album, async () => { updating = true; return null; }, async () => { online++; return "online"; }, true, () => updating);
  assert.equal(result, null);
  assert.equal(online, 0);
});

test("online artwork settings have locale parity", () => {
  for (const locale of ["en", "pt-BR"]) {
    const settings = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8")).settings;
    assert.ok(settings.onlineArtwork);
    assert.ok(settings.onlineArtworkDescription);
  }
});

const tick = () => new Promise(resolve => setImmediate(resolve));
const pending = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};

test("fast scrolling skips abandoned queued cards without increasing the two-request limit", async () => {
  const gates = [pending(), pending()], calls = [];
  const albums = Array.from({ length: 8 }, (_, index) => ({ ...album, id: String(index), folder_path: String(index), discs: [] }));
  const resolver = new LocalArtworkResolver(async path => {
    calls.push(path);
    return Number(path) < 2 ? gates[Number(path)].promise : `cover:${path}`;
  }, async () => assert.fail("online"));
  resolver.setContext(false, albums);
  let mounted = true;
  const first = albums.slice(0, 2).map(item => resolver.resolve(item, false, false));
  const abandoned = albums.slice(2, 7).map(item => resolver.resolve(item, false, false, undefined, () => mounted));
  mounted = false;
  const visible = resolver.resolve(albums[7], false, false);
  assert.deepEqual(calls, ["0", "1"]);
  gates.forEach(gate => gate.resolve("cover"));
  await Promise.all([...first, ...abandoned]);
  assert.equal(await visible, "cover:7");
  assert.deepEqual(calls, ["0", "1", "7"]);
});

test("a background or remounted consumer keeps a shared queued lookup alive", async () => {
  const gates = [pending(), pending()], calls = [];
  const albums = Array.from({ length: 3 }, (_, index) => ({ ...album, id: String(index), folder_path: String(index), discs: [] }));
  const resolver = new LocalArtworkResolver(async path => {
    calls.push(path);
    return Number(path) < 2 ? gates[Number(path)].promise : "shared";
  }, async () => null);
  let mounted = true;
  const first = albums.slice(0, 2).map(item => resolver.resolve(item, false, false));
  const old = resolver.resolve(albums[2], false, false, undefined, () => mounted);
  const background = resolver.resolve(albums[2], false, false);
  mounted = false;
  gates.forEach(gate => gate.resolve(null));
  await Promise.all(first);
  assert.equal(await background, "shared");
  assert.equal(await old, null);
  assert.deepEqual(calls, ["0", "1", "2"]);
});

test("all pending catalog states block artwork reads", () => {
  const ready = { loaded: true, loading: false, saveBlocked: false, mpdBusy: false, error: null };
  assert.equal(artworkReadBlocked(false, ready), false);
  assert.equal(artworkReadBlocked(true, ready), true);
  for (const patch of [{ loaded: false }, { loading: true }, { saveBlocked: true }, { mpdBusy: true }, { error: "failed" }]) {
    assert.equal(artworkReadBlocked(false, { ...ready, ...patch }), true);
  }
});

test("a remount during cancellation starts fresh instead of joining an already deferred lookup", async () => {
  const gate = pending();
  let calls = 0, mounted = true, remounted;
  const resolver = new LocalArtworkResolver(async () => ++calls === 1 ? gate.promise : "fresh", async () => null);
  const old = resolver.resolve(album, false, false, phase => {
    if (phase === "deferred" && !remounted) remounted = resolver.resolve(album, false, false);
  }, () => mounted);
  mounted = false;
  gate.resolve("obsolete");
  assert.equal(await old, null);
  assert.equal(await remounted, "fresh");
  assert.equal(resolver.get(album.id), "fresh");
  assert.equal(calls, 2);
});

test("queued cards reuse a cover that arrives before their slot opens", async () => {
  const gates = [pending(), pending()], calls = [];
  const albums = Array.from({ length: 3 }, (_, index) => ({ ...album, id: String(index), folder_path: String(index), discs: [] }));
  const resolver = new LocalArtworkResolver(async path => {
    calls.push(path);
    return Number(path) < 2 ? gates[Number(path)].promise : "available";
  }, async () => null);
  const first = albums.slice(0, 2).map(item => resolver.resolve(item, false, false));
  const queued = resolver.resolve(albums[2], false, false);
  assert.equal(await resolver.resolveLocalOnly(albums[2]), "available");
  gates.forEach(gate => gate.resolve(null));
  await Promise.all(first);
  assert.equal(await queued, "available");
  assert.deepEqual(calls, ["0", "1", "2"], "the queued read must reuse the session cover");
});

test("local cache reuse and LRU eviction retain the existing explicit bound", async () => {
  const calls = [];
  const albums = Array.from({ length: 3 }, (_, index) => ({ ...album, id: String(index), folder_path: String(index), discs: [] }));
  const resolver = new LocalArtworkResolver(async path => { calls.push(path); return `cover:${path}`; }, async () => null, 2);
  await resolver.resolve(albums[0], false, false);
  await resolver.resolve(albums[1], false, false);
  assert.equal(await resolver.resolve(albums[0], false, false), "cover:0");
  assert.deepEqual(calls, ["0", "1"]);
  await resolver.resolve(albums[2], false, false);
  assert.equal(resolver.get(albums[1].id), null);
  assert.equal(resolver.get(albums[0].id), "cover:0");
});

test("cached covers survive blocking; deferred lookups neither do I/O nor cache a missing result", async () => {
  let local = 0, online = 0;
  const resolver = new LocalArtworkResolver(async () => { local++; return "cover"; }, async () => { online++; return "remote"; });
  const other = { ...album, id: "other" };
  resolver.setContext(false, [album, other]);
  await resolver.resolve(album, true, false);
  resolver.setContext(true, [album, other]);
  const phases = [];
  assert.equal(await resolver.resolve(album, true, true), "cover");
  assert.equal(await resolver.resolve(other, true, false, phase => phases.push(phase)), null);
  assert.deepEqual(phases, ["deferred"]);
  assert.equal(local, 1); assert.equal(online, 0); assert.equal(resolver.get(other.id), null);
  resolver.setContext(false, [album, other]);
  assert.deepEqual(await Promise.all([resolver.resolve(other, true, false), resolver.resolve(other, true, false)]), ["cover", "cover"]);
  assert.equal(local, 2, "resume deduplicates a card and background lookup");
});

test("update removing a source discards old responses and queued work; new album resumes", async () => {
  const gate = pending(), calls = [], published = [];
  const removed = { ...album, id: "removed" }, kept = { ...album, id: "kept" };
  const resolver = new LocalArtworkResolver(async path => { calls.push(path); return gate.promise; }, async () => null);
  resolver.setContext(false, [removed]);
  resolver.subscribe(removed.id, cover => published.push(cover));
  const old = resolver.resolve(removed, false, false);
  resolver.setContext(true, [removed]);
  gate.resolve("obsolete");
  assert.equal(await old, null); assert.deepEqual(published, []);
  assert.equal(resolver.get(removed.id), null);
  resolver.setContext(false, [kept]);
  const count = calls.length;
  assert.equal(await resolver.resolve(removed, false, false), null);
  assert.equal(calls.length, count);
  assert.equal(await resolver.resolve(kept, false, false), "obsolete");
  assert.equal(resolver.get(kept.id), "obsolete");
});

test("bounded resume avoids bursts and queued stale albums never start I/O", async () => {
  const gates = [pending(), pending(), pending()], calls = [];
  const albums = gates.map((_, index) => ({ ...album, id: String(index), folder_path: String(index), discs: [] }));
  const resolver = new LocalArtworkResolver(async path => { calls.push(path); return gates[Number(path)].promise; }, async () => null);
  resolver.setContext(false, albums);
  const reads = albums.map(item => resolver.resolve(item, false, false));
  await tick(); assert.deepEqual(calls, ["0", "1"]);
  resolver.setContext(true, albums);
  gates[0].resolve("old"); gates[1].resolve("old");
  assert.deepEqual(await Promise.all(reads), [null, null, null]);
  assert.deepEqual(calls, ["0", "1"]);
});

test("updating mid-disc lookup stops subsequent local and online queries", async () => {
  let blocked = false, calls = 0;
  const phases = [];
  assert.equal(await lookupAlbumArtwork(album, async () => { calls++; blocked = true; return "stale"; }, async () => assert.fail("online"), true, () => blocked, undefined, phase => phases.push(phase)), null);
  assert.equal(calls, 1); assert.deepEqual(phases, ["deferred"]);
});

test("resolveLocalOnly stays independent of the browsing suspension", async () => {
  let local = 0;
  const resolver = new LocalArtworkResolver(async () => { local++; return "local"; }, async () => assert.fail("online"));
  resolver.setContext(true, []);
  assert.equal(await resolver.resolveLocalOnly(album), "local");
  assert.equal(local, 1);
});

test("a shared in-flight lookup propagates deferral to every observer", async () => {
  let blocked = false;
  const gate = pending(), phases = [];
  const resolver = new LocalArtworkResolver(async () => gate.promise, async () => null);
  const card = resolver.resolve(album, false, () => blocked);
  const background = resolver.resolve(album, false, false, phase => phases.push(phase));
  blocked = true; gate.resolve(null);
  assert.deepEqual(await Promise.all([card, background]), [null, null]);
  assert.deepEqual(phases, ["deferred"]);
});

test("cards and selected album retain covers, invalidate obsolete reads and resume on the existing gate", () => {
  const view = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  assert.match(view, /isLibraryUpdating=\{artworkBlocked\}/);
  assert.match(view, /catalog\.albums !== version \|\| !catalog\.albums\.includes\(album\)/);
  assert.match(view, /if \(!selectedAlbum \|\| artworkBlocked\) return/);
  assert.match(view, /if \(active && cover\) setAlbumCover\(cover\)/);
  assert.equal((view.match(/getLocalAlbumCover\([^\n]+, \(\) => active\)/g) ?? []).length, 2, "both cards and album details must invalidate their consumer on cleanup");
  assert.match(view, /return \(\) => \{ active = false; \};\s*\}, \[selectedAlbum, artworkBlocked\]/);
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /localArtworkResolver\.setContext\(blocked, localCatalog\.albums\)/);
  assert.match(app, /localArtworkEnrichment\.setUpdating\(blocked\)/);
  assert.match(app, /useLayoutEffect\(\(\) => \{\s*\/\/ Publish the artwork gate/);
});
