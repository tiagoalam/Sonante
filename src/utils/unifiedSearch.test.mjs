import test from "node:test";
import assert from "node:assert/strict";
import { SearchLane, searchQuery, localSearchIndex, localTrackRows, plexSearchPreview, searchTrackMetadata, sortSearchRows } from "./unifiedSearch.ts";
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const album = (id, title, artist = "Artist") => ({ id, title, artist, folder_path: `Source/${id}`, source_id: "Source", track_count: 1, discs: [] });

test("unified search rejects empty, short, oversized and MPD control input", () => {
  for (const query of ["", " ", "a", "ok\nclear", "ok\0play", "x".repeat(161)]) assert.equal(searchQuery(query), null);
  assert.equal(searchQuery("  live  "), "live"); assert.equal(searchQuery("á".repeat(160)), "á".repeat(160));
});
test("one normalized Local index finds both album and artist, ranks exact names and keeps album identities distinct", () => {
  const albums = [album("one", "Live", "Björk"), album("two", "Live at home", "Björk"), album("three", "Other", "Elsewhere"), album("four", "Live", "Another")];
  const find = localSearchIndex(albums);
  assert.equal(find("").rows.length, 0);
  assert.deepEqual(find("live").rows.map(row => row.album.id), ["four", "one", "two"]);
  const artist = find("Björk").rows.find(row => row.kind === "artist"); assert.equal(artist.artist.albumCount, 2);
  assert.equal(find("Björk live").rows.filter(row => row.kind === "album").length, 2);
  assert.equal(albums[0].title, "Live");
  const many = localSearchIndex(Array.from({ length: 31 }, (_, i) => album(String(i), `Live ${i}`)))("live");
  assert.equal(many.rows.filter(row => row.kind === "album").length, 20); assert.equal(many.limited, true);
});
test("same-title Local/Plex editions remain independently actionable instead of being deduplicated across origins", () => {
  const local = localSearchIndex([album("one", "Live")])("live").rows;
  const plex = plexSearchPreview({ artists: [], tracks: [], albums: [{ rating_key: "one", title: "Live", artist: "Artist", server_id: "server" }] }, "server");
  assert.equal(sortSearchRows([...local, ...plex.rows], "live").length, 2);
  assert.notEqual(local[0].key, plex.rows[0].key);
  assert.throws(() => plexSearchPreview({ artists: [], tracks: [], albums: [{ rating_key: "x", server_id: "other" }] }, "server"));
});
test("track actions preserve CUE URI, mapped Plex locator and millisecond duration without guessing paths", () => {
  const [row] = localTrackRows([{ item_type: "file", path: "Source/Album/image.cue/track0001", name: "Cue", duration: 1801 }]);
  assert.deepEqual(searchTrackMetadata(row).media_locator, { kind: "local", uri: "Source/Album/image.cue/track0001" });
  assert.equal(searchTrackMetadata(row).duration, 1801);
  const media_locator = { kind: "local", uri: "Mapped/one.flac" };
  const preview = plexSearchPreview({ albums: [], artists: [], tracks: [{ rating_key: "t", title: "Track", artist: "Artist", album_title: "Album", media_locator, duration_ms: 123456 }] }, "server");
  assert.equal(preview.rows[0].source, "plex"); assert.equal(searchTrackMetadata(preview.rows[0]).media_locator, media_locator);
  assert.equal(searchTrackMetadata(preview.rows[0]).duration, 123.456);
  assert.throws(() => plexSearchPreview({ albums: [], artists: [], tracks: [{ rating_key: "t", media_locator: { kind: "plex", server_id: "other", part_key: "/part" } }] }, "server"));
});
test("each search lane has one running request and keeps only the newest pending query", async () => {
  const lane = new SearchLane(), first = deferred(), last = deferred(), calls = [];
  lane.request("A", () => { calls.push("A"); return first.promise; }); await flush();
  lane.request("B", async () => { calls.push("B"); return "B"; });
  lane.request("C", () => { calls.push("C"); return last.promise; });
  assert.deepEqual(calls, ["A"]); first.resolve("old A"); await flush();
  assert.deepEqual(calls, ["A", "C"]); assert.equal(lane.getSnapshot().value, null);
  last.resolve("current C"); await flush(); assert.equal(lane.getSnapshot().value, "current C");
  lane.request("C", async () => assert.fail("completed query should be reused in this context")); await flush();
});
test("query clear and source/account replacement discard stale success/error and queued work", async () => {
  const errors = [], lane = new SearchLane(error => errors.push(error)), old = deferred();
  lane.request("before", () => old.promise); await flush();
  lane.request("queued", async () => assert.fail("cancelled queued load")); lane.clear();
  old.reject(Error("private old error")); await flush(); assert.deepEqual(errors, []); assert.equal(lane.getSnapshot().phase, "idle");
  lane.request("skip", async () => assert.fail("cleared before dispatch")); lane.clear(); await flush();
  lane.request("after", async () => "new account"); await flush(); assert.equal(lane.getSnapshot().value, "new account");
});
test("one unavailable source never prevents the other lane from publishing its result", async () => {
  const local = new SearchLane(), plex = new SearchLane();
  local.request("live", async () => "Local matches"); plex.request("live", async () => { throw Error("network unavailable"); });
  await flush(); assert.equal(local.getSnapshot().value, "Local matches"); assert.equal(plex.getSnapshot().phase, "error");
  assert.equal(plex.getSnapshot().value, null);
});
