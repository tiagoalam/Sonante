import test from "node:test";
import assert from "node:assert/strict";
import { PlexCatalogCoordinator } from "./plexCatalogCoordinator.ts";

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
const allAlbums = snapshot => snapshot.libraries.filter(l=>l.phase==="complete").flatMap(l=>l.albums);
const loadMany = (c,libraries) => { for(const l of libraries)void c.loadLibrary(l).catch(()=>{}); };

test("two libraries share browser promises, preserve provenance and reuse complete session inventory", async () => {
  const { coordinator: c, calls } = fixture();
  const browser = c.loadLibrary(library("1"));
  loadMany(c,[library("1"), library("2")]);
  assert.equal(c.loadLibrary(library("1")), browser);
  await flush(); assert.deepEqual(calls.map(call => call.key), ["1", "2"]);
  calls[0].resolve([album("same")]); await browser; await flush();
  const first = c.getSnapshot(); assert.equal(allAlbums(first).length, 1);
  calls[1].resolve([album("same")]); await flush();
  const complete = c.getSnapshot(); assert.equal(allAlbums(complete).length, 2);
  assert.deepEqual(allAlbums(complete).map(a => [a.server_id, a.library_id, a.rating_key]), [["server", "1", "same"], ["server", "2", "same"]]);
  assert.equal(allAlbums(complete)[0], allAlbums(first)[0]);
  loadMany(c,[library("1"), library("2")]); await c.loadLibrary(library("2")); await flush();
  assert.equal(calls.length, 2); assert.equal(c.getSnapshot(), complete);
  c.invalidate();
});

test("queue runs at most two libraries and partial failure does not hide success; retry only reloads failures", async () => {
  const { coordinator: c, calls, errors } = fixture();
  loadMany(c,[library("1"), library("2"), library("3")]); await flush();
  assert.equal(calls.length, 2); assert.equal(c.getSnapshot().libraries[2].phase, "queued");
  calls[0].resolve([album("1")]); await flush(); assert.equal(calls.length, 3);
  const available = c.getSnapshot().libraries[0].albums;
  calls[1].reject({ completeness: "partial", received: 200, message: "HTTP 500" }); await flush();
  assert.equal(c.getSnapshot().libraries[0].albums, available); assert.equal(errors.length, 1);
  calls[2].resolve([album("3")]); await flush();
  assert.deepEqual(allAlbums(c.getSnapshot()).map(a => a.library_id), ["1", "3"]);
  loadMany(c,[library("1"), library("2"), library("3")]); await flush(); assert.equal(calls.length, 3);
  void c.loadLibrary(library("2"), true).catch(()=>{}); await flush(); assert.deepEqual(calls.map(call => call.key), ["1", "2", "3", "2"]);
  calls[3].resolve([album("2")]); await flush();
  assert.ok(c.getSnapshot().libraries.every(l => l.phase === "complete"));
  assert.equal(allAlbums(c.getSnapshot()).length, 3); c.invalidate();
});

test("logout/server changes discard obsolete data and queued work while old requests retain concurrency slots", async () => {
  for (const change of ["logout", "server", "account"]) {
    const { coordinator: c, calls, identity } = fixture();
    loadMany(c,[library("1"), library("2"), library("old-queued")]); await flush();
    if (change === "logout") c.invalidate(identity);
    else c.setContext({}, change === "server" ? "other-server" : "server");
    assert.equal(allAlbums(c.getSnapshot()).length, 0);
    if (change !== "logout") loadMany(c,[library("new")]);
    await flush(); assert.equal(calls.length, 2);
    calls[0].resolve([album("obsolete")]); calls[1].reject(new Error("obsolete")); await flush();
    assert.equal(allAlbums(c.getSnapshot()).length, 0);
    assert.ok(!calls.some(call => call.key === "old-queued"));
    if (change !== "logout") {
      assert.equal(calls.length, 3); calls[2].resolve([album("fresh")]); await flush();
      assert.deepEqual(allAlbums(c.getSnapshot()).map(a => a.title), ["Album fresh"]);
      assert.equal(allAlbums(c.getSnapshot())[0].server_id, change === "server" ? "other-server" : "server");
    }
    c.invalidate();
  }
});
