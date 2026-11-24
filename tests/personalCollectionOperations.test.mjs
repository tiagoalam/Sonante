import test from "node:test";
import assert from "node:assert/strict";
import {
  associateCollectionAlbums,
  collectionSelectionKey,
  resolveCollectionMembers,
  toggleCollectionSelection,
} from "../src/utils/personalCollectionOperations.mjs";

const local = (albumId, title = "Same title") => ({ source: "local", albumId, title, artist: "Artist" });
const plex = (serverId, ratingKey) => ({ source: "plex", serverId, libraryId: "music", ratingKey, title: "Album", artist: "Artist" });
const subsonic = (connectionId, albumId) => ({ source: "subsonic", connectionId, accountId: "account", albumId, title: "Album", artist: "Artist" });

test("selection is multi-source, scoped and toggles only the exact source identity", () => {
  const first = local("sqlite:1:one");
  const sameName = local("sqlite:2:two");
  const selected = toggleCollectionSelection(toggleCollectionSelection(toggleCollectionSelection([], first), sameName), plex("server", "42"));
  assert.equal(selected.length, 3);
  assert.notEqual(collectionSelectionKey(first), collectionSelectionKey(sameName));
  assert.equal(toggleCollectionSelection(selected, first).length, 2);
  assert.notEqual(collectionSelectionKey(plex("server-a", "42")), collectionSelectionKey(plex("server-b", "42")));
  assert.notEqual(collectionSelectionKey(subsonic("connection-a", "42")), collectionSelectionKey(subsonic("connection-b", "42")));
});

test("association reports per-album success, duplicate and failure without claiming global success", async () => {
  const items = [local("a"), plex("server", "b"), subsonic("conn", "c")];
  const outcomes = await associateCollectionAlbums(items, async album => {
    if (album.source === "plex") throw "COLLECTION_DUPLICATE_MEMBER";
    if (album.source === "subsonic") throw "COLLECTION_SOURCE_UNAVAILABLE";
  });
  assert.deepEqual(outcomes.map(item => item.status), ["added", "duplicate", "failed"]);
  assert.equal(outcomes[2].code, "COLLECTION_SOURCE_UNAVAILABLE");
});

test("stale resolution publishes nothing after collection navigation changes", async () => {
  const members = [{ id: 1 }, { id: 2 }, { id: 3 }];
  let current = true;
  let publishCount = 0;
  let resolveFirst;
  const first = new Promise(resolve => { resolveFirst = resolve; });
  const operation = resolveCollectionMembers(members, member => member.id === 1 ? first : Promise.resolve({ status: "available" }),
    () => current, () => { publishCount++; }, 1);
  current = false;
  resolveFirst({ status: "available" });
  await operation;
  assert.equal(publishCount, 0);
});

test("resolution preserves source statuses and reports rejected IPC separately", async () => {
  const members = [{ id: 1 }, { id: 2 }, { id: 3 }];
  const results = new Map();
  await resolveCollectionMembers(members, member => {
    if (member.id === 3) return Promise.reject(new Error("storage"));
    return Promise.resolve({ status: member.id === 1 ? "available" : "unavailable", target: null });
  }, () => true, (id, result, error) => results.set(id, { result, error }), 2);
  assert.equal(results.get(1).result.status, "available");
  assert.equal(results.get(2).result.status, "unavailable");
  assert.equal(results.get(3).error, "error");
});

test("a partial attempt reports each original album once and retries only failures", async () => {
  const items = [local("one"), local("two"), local("three"), local("four")];
  const first = await associateCollectionAlbums(items, async album => {
    if (album.albumId !== "one") throw "COLLECTION_SOURCE_UNAVAILABLE";
  });
  assert.equal(first.length, 4);
  assert.equal(first.filter(result => result.status === "added").length, 1);
  const failed = first.filter(result => result.status === "failed").map(result => result.album);
  assert.equal(failed.length, 3);
  const retry = await associateCollectionAlbums(failed, async () => {});
  assert.equal(retry.length, 3);
  assert.ok(retry.every(result => result.status === "added"));
});

test("retry merges outcomes without losing already confirmed additions or duplicates", async () => {
  const { mergeCollectionOutcomes } = await import("../src/utils/personalCollectionOperations.mjs");
  const first = [{album:local("one"),status:"added"},{album:local("two"),status:"duplicate"},{album:local("three"),status:"failed"}];
  const next = mergeCollectionOutcomes(first,[{album:local("three"),status:"added"}]);
  assert.deepEqual(next.map(item => item.status),["added","duplicate","added"]);
});

test("visible resolution queue bounds concurrency and skips offscreen or disposed work", async () => {
  const {createVisibleResolutionQueue} = await import("../src/utils/personalCollectionOperations.mjs");
  let active=0, peak=0, calls=[]; const pending=[]; const published=[];
  const queue=createVisibleResolutionQueue(member=>{ calls.push(member.id); peak=Math.max(peak,++active); return new Promise(resolve=>pending.push(()=>{active--;resolve(member.id);})); }, (id)=>published.push(id), 2);
  let visible=true;
  queue.enqueue({id:1},()=>true); queue.enqueue({id:2},()=>true); queue.enqueue({id:3},()=>visible); queue.enqueue({id:4},()=>true);
  await new Promise(resolve=>setImmediate(resolve)); assert.deepEqual(calls,[1,2]); visible=false;
  pending.shift()(); await new Promise(resolve=>setImmediate(resolve)); assert.deepEqual(calls,[1,2,4]);
  queue.dispose(); pending.splice(0).forEach(done=>done()); await new Promise(resolve=>setImmediate(resolve));
  assert.equal(peak,2); assert.deepEqual(published,[1]);
});
