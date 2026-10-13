import test from "node:test";
import assert from "node:assert/strict";
import { PlexCatalogCoordinator } from "./plexCatalogCoordinator.ts";
import { createExploreSession, deriveExploreGroups, exploreEntryIsCurrent } from "./explore.ts";

const library = key => ({ key, title: `Library ${key}` });
const album = (id, artist = "Artist") => ({ rating_key: id, artist_rating_key: "artist", title: `Album ${id}`, artist, genres: ["Rock"] });
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const flush = async () => { for (let i = 0; i < 24; i++) await Promise.resolve(); };
function fixture() {
  const calls = [], errors = [];
  const coordinator = new PlexCatalogCoordinator(key => { const d = deferred(); calls.push({ key, ...d }); return d.promise; }, error => errors.push(error));
  const identity = {}; coordinator.setContext(identity, "server");
  return { coordinator, calls, errors, identity };
}
const context = (coordinator, local = []) => {
  const s = coordinator.getSnapshot();
  return { local: { albums: local, state: local.length ? "ready" : "empty" }, plex: { albums: s.albums, catalogs: s.catalogs, identity: s.identity, serverId: s.serverId,
    libraryId: "", state: s.albums.length ? "ready" : "loading", loading: s.libraries.some(l => ["queued", "loading"].includes(l.phase)) } };
};

test("two libraries share browser/Explore promises, preserve provenance and reuse complete session inventory", async () => {
  const { coordinator: c, calls } = fixture();
  const browser = c.loadLibrary(library("1"));
  c.aggregate([library("1"), library("2")]);
  assert.equal(c.loadLibrary(library("1")), browser);
  await flush(); assert.deepEqual(calls.map(call => call.key), ["1", "2"]);
  calls[0].resolve([album("same")]); await browser; await flush();
  const first = c.getSnapshot(); assert.equal(first.albums.length, 1);
  calls[1].resolve([album("same")]); await flush();
  const complete = c.getSnapshot(); assert.equal(complete.albums.length, 2);
  assert.deepEqual(complete.albums.map(a => [a.server_id, a.library_id, a.rating_key]), [["server", "1", "same"], ["server", "2", "same"]]);
  assert.equal(complete.albums[0], first.albums[0]);
  c.aggregate([library("1"), library("2")]); await c.loadLibrary(library("2")); await flush();
  assert.equal(calls.length, 2); assert.equal(c.getSnapshot(), complete);
  c.invalidate();
});

test("queue runs at most two libraries and partial failure does not hide success; retry only reloads failures", async () => {
  const { coordinator: c, calls, errors } = fixture();
  c.aggregate([library("1"), library("2"), library("3")]); await flush();
  assert.equal(calls.length, 2); assert.equal(c.getSnapshot().libraries[2].phase, "queued");
  calls[0].resolve([album("1")]); await flush(); assert.equal(calls.length, 3);
  const available = c.getSnapshot().albums;
  calls[1].reject({ completeness: "partial", received: 200, message: "HTTP 500" }); await flush();
  assert.equal(c.getSnapshot().albums, available); assert.equal(errors.length, 1);
  calls[2].resolve([album("3")]); await flush();
  assert.deepEqual(c.getSnapshot().albums.map(a => a.library_id), ["1", "3"]);
  c.aggregate([library("1"), library("2"), library("3")]); await flush(); assert.equal(calls.length, 3);
  c.retryFailures(); await flush(); assert.deepEqual(calls.map(call => call.key), ["1", "2", "3", "2"]);
  calls[3].resolve([album("2")]); await flush();
  assert.ok(c.getSnapshot().libraries.every(l => l.phase === "complete"));
  assert.equal(c.getSnapshot().albums.length, 3); c.invalidate();
});

test("logout/server changes discard obsolete data and queued work while old requests retain concurrency slots", async () => {
  for (const change of ["logout", "server", "account"]) {
    const { coordinator: c, calls, identity } = fixture();
    c.aggregate([library("1"), library("2"), library("old-queued")]); await flush();
    if (change === "logout") c.invalidate(identity);
    else c.setContext({}, change === "server" ? "other-server" : "server");
    assert.equal(c.getSnapshot().albums.length, 0);
    if (change !== "logout") c.aggregate([library("new")]);
    await flush(); assert.equal(calls.length, 2);
    calls[0].resolve([album("obsolete")]); calls[1].reject(new Error("obsolete")); await flush();
    assert.equal(c.getSnapshot().albums.length, 0);
    assert.ok(!calls.some(call => call.key === "old-queued"));
    if (change !== "logout") {
      assert.equal(calls.length, 3); calls[2].resolve([album("fresh")]); await flush();
      assert.deepEqual(c.getSnapshot().albums.map(a => a.title), ["Album fresh"]);
      assert.equal(c.getSnapshot().albums[0].server_id, change === "server" ? "other-server" : "server");
    }
    c.invalidate();
  }
});

test("aggregate additions preserve cards, valid details and pages; artist slices keep library scope and genres span libraries", async () => {
  const { coordinator: c, calls } = fixture();
  c.aggregate([library("1"), library("2")]); await flush();
  const local = Array.from({ length: 8 }, (_, i) => ({ id: `L${i}`, title: `Local ${i}`, artist: "Local", folder_path: "Synthetic", discs: [], track_count: 1, genres: ["Rock"] }));
  calls[0].resolve(Array.from({ length: 8 }, (_, i) => album(String(i)))); await flush();
  const s = createExploreSession(() => 0.5); s.syncContext(context(c, local), true);
  const chosen = s.getSnapshot().selected.map(e => e.key), detail = s.getSnapshot().selected.find(e => e.source === "plex");
  s.openSlice(s.getSnapshot().genres[0], s.getSnapshot().revision); s.setPage(1, s.getSnapshot().revision);
  const page = s.sliceEntries().map(e => e.key);
  calls[1].resolve(Array.from({ length: 8 }, (_, i) => album(String(i)))); await flush();
  const full = context(c, local); s.syncContext(full, true);
  assert.deepEqual(s.getSnapshot().selected.map(e => e.key), chosen);
  assert.deepEqual(s.sliceEntries().map(e => e.key), page); assert.equal(s.getSnapshot().slice.entries.length, 24);
  assert.ok(exploreEntryIsCurrent(full, detail));
  const selectedLibrary = "1";
  const groups = deriveExploreGroups(full); assert.equal(groups.artists.filter(a => a.source === "plex").length, 2);
  s.leaveSlice(s.getSnapshot().revision);
  assert.ok(!s.getSnapshot().artists.some(a => a.libraryId === "2"), "late libraries do not replace the published artist shelf");
  s.renew();
  const other = s.getSnapshot().artists.find(a => a.libraryId === "2"); assert.ok(other);
  s.openSlice(other, s.getSnapshot().revision);
  assert.equal(s.getSnapshot().slice.entries.length, 8); assert.ok(s.sliceEntries().every(e => e.libraryId === "2"));
  assert.ok(s.sliceEntries().every(e => exploreEntryIsCurrent(full, e))); assert.equal(selectedLibrary, "1");
  s.leaveSlice(s.getSnapshot().revision); s.renew();
  assert.equal(s.candidateCount(), 24); assert.equal(s.visibleEntries().length, 10);
  s.syncContext(full, false); s.syncContext(full, true); assert.equal(calls.length, 2);
  c.invalidate(); assert.equal(exploreEntryIsCurrent(context(c, local), detail), false);
});

test("finishing a failed library preserves an available distinct shelf and its current slice", () => {
  const local = Array.from({ length: 8 }, (_, i) => ({ id: `L${i}`, title: `Local ${i}`, artist: `Artist ${i}`, folder_path: "Synthetic", discs: [], track_count: 1, genres: ["Rock"] }));
  const plex = [album("1")], identity = {};
  const c = { local: { albums: local, state: "ready" }, plex: { albums: plex, state: "ready", identity, serverId: "server", libraryId: "1", loading: true } };
  const s = createExploreSession(() => 0.5); s.syncContext(c, true); assert.equal(s.visibleEntries().length, 9);
  const first = s.visibleEntries().map(e => e.key);
  s.openSlice(s.getSnapshot().genres[0], s.getSnapshot().revision); const slice = s.getSnapshot().slice;
  s.syncContext({ ...c, plex: { ...c.plex, loading: false } }, true);
  assert.equal(s.visibleEntries().length, 9); assert.deepEqual(s.visibleEntries().map(e => e.key), first);
  assert.equal(s.getSnapshot().slice, slice);
});
