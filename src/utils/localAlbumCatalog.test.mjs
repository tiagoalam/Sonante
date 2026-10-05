import test from "node:test";
import assert from "node:assert/strict";
import { LocalAlbumCatalog, emptyLocalAlbumCatalogState } from "./localAlbumCatalog.ts";

const album = (id) => ({ id, title: id, artist: "Artist", year: null, folder_path: id, track_count: 1, discs: [] });
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("cached catalog survives update and remount without another request", async () => {
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => { calls += 1; return [album("old")]; });
  catalog.setUpdating(false, true);
  await tick();
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  catalog.setUpdating(true);
  catalog.setUpdating(true, true);
  assert.equal(calls, 1);
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), true), null);
});

test("mount during update without cache shows indexing, then refreshes once", async () => {
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => { calls += 1; return []; });
  catalog.setUpdating(true, true);
  assert.equal(calls, 0);
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), true), "indexing");
  catalog.setUpdating(false);
  catalog.setUpdating(false);
  await tick();
  await tick();
  assert.equal(calls, 1);
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "empty");
});

test("stale response cannot replace the newer catalog and requests do not overlap", async () => {
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise((resolve) => pending.push(resolve)));
  catalog.setUpdating(false, true);
  await tick();
  assert.equal(pending.length, 1);
  catalog.setUpdating(true);
  catalog.setUpdating(false);
  assert.equal(pending.length, 1);
  pending[0]([album("stale")]);
  await tick();
  assert.equal(pending.length, 2);
  assert.deepEqual(catalog.getSnapshot().albums, []);
  pending[1]([album("new")]);
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["new"]);
});

test("remount waits for an existing request before refreshing", async () => {
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise((resolve) => pending.push(resolve)));
  catalog.setUpdating(false, true);
  await tick();
  catalog.setUpdating(false, true);
  assert.equal(pending.length, 1);
  pending[0]([album("old")]);
  await tick();
  assert.equal(pending.length, 2);
  pending[1]([album("new")]);
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["new"]);
});

test("real load error remains visible and does not clear the cached catalog", async () => {
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => {
    calls += 1;
    if (calls === 1) return [album("old")];
    throw new Error("MPD unavailable");
  });
  catalog.setUpdating(false, true);
  await tick();
  catalog.setUpdating(true);
  catalog.setUpdating(false);
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  assert.equal(catalog.getSnapshot().error, "MPD unavailable");
});

test("initial load failure is an error state rather than an empty library", async () => {
  const catalog = new LocalAlbumCatalog(async () => { throw new Error("MPD unavailable"); });
  catalog.setUpdating(false, true);
  await tick();
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "error");
});
