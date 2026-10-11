import test from "node:test";
import assert from "node:assert/strict";
import {
  LocalAlbumCatalog,
  emptyLocalAlbumCatalogState,
  localCatalogActivity,
  refreshCatalogAfterSuccessfulCheck,
  refreshCatalogAfterSuccessfulRescan,
} from "./localAlbumCatalog.ts";

const album = (id) => ({ id, title: id, artist: "Artist", year: null, folder_path: id, track_count: 1, discs: [] });
const tick = () => new Promise((resolve) => setImmediate(resolve));
const mpdBusy = "Biblioteca local em atualização no MPD; tente novamente após a atualização.";

test("MPD busy on initial load stays transient until the existing status sample permits a complete retry", async () => {
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => {
    calls += 1;
    if (calls === 1) throw new Error(mpdBusy);
    return [album("complete")];
  });

  catalog.setUpdating(false, true);
  await tick();
  assert.equal(catalog.getSnapshot().error, null);
  assert.equal(catalog.getSnapshot().mpdBusy, true);
  assert.deepEqual(catalog.getSnapshot().albums, []);
  assert.equal(localCatalogActivity(false, catalog.getSnapshot()), "indexing");
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "indexing");

  catalog.onMpdStatus(true);
  catalog.onMpdStatus(true);
  assert.equal(calls, 1);
  catalog.onMpdStatus(false);
  await tick();
  assert.equal(calls, 2);
  assert.equal(catalog.getSnapshot().mpdBusy, false);
  assert.equal(catalog.getSnapshot().error, null);
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["complete"]);
});

test("a busy response missed by the status poll retries on its next idle sample", async () => {
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => {
    calls += 1;
    if (calls === 1) throw mpdBusy;
    return [];
  });
  catalog.setUpdating(false, true);
  await tick();
  catalog.onMpdStatus(false);
  await tick();
  assert.equal(calls, 2);
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "empty");
});

test("busy during refresh retains confirmed albums and never publishes a partial catalog", async () => {
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => {
    calls += 1;
    if (calls === 1) return [album("old")];
    if (calls === 2) throw new Error(mpdBusy);
    return [album("new")];
  });
  catalog.setUpdating(false, true);
  await tick();
  catalog.requestRefresh();
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  assert.equal(catalog.getSnapshot().error, null);
  assert.equal(localCatalogActivity(false, catalog.getSnapshot()), "indexing");
  catalog.onMpdStatus(false);
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["new"]);
});

test("only the exact MPD indexing response is treated as transient", async () => {
  const detail = `${mpdBusy} Falha de transporte.`;
  const catalog = new LocalAlbumCatalog(async () => { throw new Error(detail); });
  catalog.setUpdating(false, true);
  await tick();
  assert.equal(catalog.getSnapshot().error, detail);
  assert.equal(catalog.getSnapshot().mpdBusy, false);
  assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "error");
});

test("activity follows MPD indexing and catalog organization while cached albums stay visible", async () => {
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise((resolve) => pending.push(resolve)));

  catalog.setUpdating(true, true);
  assert.equal(localCatalogActivity(true, catalog.getSnapshot()), "indexing");
  catalog.setUpdating(false);
  await tick();
  assert.equal(localCatalogActivity(false, catalog.getSnapshot()), "organizing");
  pending[0]([album("old")]);
  await tick();
  assert.equal(localCatalogActivity(false, catalog.getSnapshot()), null);

  catalog.setUpdating(true);
  assert.equal(localCatalogActivity(true, catalog.getSnapshot()), "indexing");
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  catalog.setUpdating(false);
  await tick();
  assert.equal(localCatalogActivity(false, catalog.getSnapshot()), "organizing");
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  pending[1]([album("new")]);
  await tick();
  assert.equal(localCatalogActivity(false, catalog.getSnapshot()), null);
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["new"]);
});

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

test("successful rescan notifies once and failed rescan never notifies", async () => {
  let notifications = 0;
  await refreshCatalogAfterSuccessfulRescan(
    async () => {},
    () => { notifications += 1; },
  );
  assert.equal(notifications, 1);

  await assert.rejects(
    refreshCatalogAfterSuccessfulRescan(
      async () => { throw new Error("rescan failed"); },
      () => { notifications += 1; },
    ),
    /rescan failed/,
  );
  assert.equal(notifications, 1);
});

test("incremental check refreshes only after a confirmed change", async () => {
  let notifications = 0;
  const notify = () => { notifications += 1; };

  assert.equal(await refreshCatalogAfterSuccessfulCheck(async () => false, notify), false);
  assert.equal(notifications, 0);
  assert.equal(await refreshCatalogAfterSuccessfulCheck(async () => true, notify), true);
  assert.equal(notifications, 1);
  await assert.rejects(
    refreshCatalogAfterSuccessfulCheck(async () => { throw new Error("refresh failed"); }, notify),
    /refresh failed/,
  );
  assert.equal(notifications, 1);
});

test("manual refresh keeps cached albums and publishes the fresh catalog", async () => {
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise((resolve) => pending.push(resolve)));
  catalog.setUpdating(false, true);
  await tick();
  pending[0]([album("old")]);
  await tick();

  catalog.requestRefresh();
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["old"]);
  assert.equal(catalog.getSnapshot().loading, true);
  pending[1]([album("new")]);
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["new"]);
});

test("manual refresh during a load discards stale result and serializes the next load", async () => {
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise((resolve) => pending.push(resolve)));
  catalog.setUpdating(false, true);
  await tick();
  catalog.requestRefresh();
  assert.equal(pending.length, 1);

  pending[0]([album("stale")]);
  await tick();
  assert.equal(pending.length, 2);
  assert.deepEqual(catalog.getSnapshot().albums, []);

  pending[1]([album("fresh")]);
  await tick();
  assert.deepEqual(catalog.getSnapshot().albums.map((item) => item.id), ["fresh"]);
});
