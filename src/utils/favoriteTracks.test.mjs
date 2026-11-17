import test from "node:test";
import assert from "node:assert/strict";
import { favoriteLocalTracks } from "./favoriteTracks.ts";

test("SQLite album favorite resolves by opaque album identity and returns source-bound track locators", async () => {
  const album = { id: "sqlite:42:album-id", folder_path: "sqlite-album://42/album-id", title: "Synthetic", artist: "Fixture" };
  const calls = [];
  const service = {
    async getLocalAlbums() { return [album]; },
    async getLocalAlbumTracks(id) {
      calls.push(["tracks", id]);
      return [{ number: 1, tracks: [{ path: "sqlite-track://42/YWxidW0vMDEud2F2", title: "Track", artist: "Fixture" }] }];
    },
    async listLocalDirectory(path) { calls.push(["directory", path]); return []; },
  };
  const tracks = await favoriteLocalTracks({ id: album.id, source: "local", title: album.title, artist: album.artist,
    path_or_key: album.folder_path }, service);
  assert.equal(calls[0][0], "tracks");
  assert.equal(calls[0][1], album.id);
  assert.equal(calls.some(([kind]) => kind === "directory"), false);
  assert.equal(tracks[0].uri, "sqlite-track://42/YWxidW0vMDEud2F2");
});

test("CUE SQLite favorite restores virtual segment locators without a directory fallback", async () => {
  const album = { id: "sqlite-cue-album:42:c2V0", folder_path: "sqlite-album://sqlite-cue-album:42/c2V0", title: "CUE Set", artist: "Fixture" };
  const calls = [];
  const service = {
    async getLocalAlbums() { return [album]; },
    async getLocalAlbumTracks(id) {
      calls.push(["tracks", id]);
      return [{ number: 1, tracks: [{ path: "sqlite-cue://42/YWxidW0uY3Vl/2", title: "Segment 2", artist: "Fixture" }] }];
    },
    async listLocalDirectory(path) { calls.push(["directory", path]); return []; },
  };
  const tracks = await favoriteLocalTracks({ id: album.id, source: "local", title: album.title, artist: album.artist,
    path_or_key: album.folder_path }, service);
  assert.deepEqual(calls, [["tracks", album.id]]);
  assert.equal(tracks[0].uri, "sqlite-cue://42/YWxidW0uY3Vl/2");
});

test("a removed or changed SQLite favorite never falls through to MPD directory lookup", async () => {
  const calls = [];
  const stillPresentAtSamePath = { id: "sqlite:42:new-identity", folder_path: "sqlite-album://42/new-identity", title: "Old", artist: "Fixture" };
  const service = {
    async getLocalAlbums() { return [stillPresentAtSamePath]; },
    async getLocalAlbumTracks() { throw new Error("must not be called"); },
    async listLocalDirectory(path) { calls.push(path); return []; },
  };
  await assert.rejects(favoriteLocalTracks({ id: "sqlite:42:old", source: "local", title: "Old", artist: "Fixture",
    path_or_key: "sqlite-album://42/old" }, service), { message: "SQLITE_FAVORITE_REFERENCE_UNAVAILABLE" });
  assert.deepEqual(calls, []);
});

test("legacy Local path favorites remain path-resolvable even when their ID resembles SQLite", async () => {
  const favorite = { id: "sqlite:legacy-folder", source: "local", title: "Legacy Album", artist: "Fixture",
    path_or_key: "Music/Legacy Album", exists: true };
  const calls = [];
  const service = {
    async getLocalAlbums() { return [{ id: "mpd-album-id", folder_path: favorite.path_or_key, title: favorite.title }]; },
    async getLocalAlbumTracks(id) { calls.push(["tracks", id]); return [{ number: 1, tracks: [{ path: "Music/Legacy Album/01.flac", title: "Track" }] }]; },
    async listLocalDirectory(path) { calls.push(["directory", path]); return []; },
  };
  const tracks = await favoriteLocalTracks(favorite, service);
  assert.deepEqual(calls, [["tracks", "mpd-album-id"]]);
  assert.equal(tracks[0].uri, "Music/Legacy Album/01.flac");
});

test("unresolved legacy favorite is not sent to the MPD directory lookup in SQLite mode", async () => {
  const calls = [];
  const favorite = { id: "old-mpd-album-id", source: "local", title: "Legacy Album", artist: "Fixture",
    path_or_key: "Music/Legacy Album", exists: true };
  const service = {
    async getLocalAlbums() { return [{ id: "sqlite:42:different", folder_path: "sqlite-album://42/different", title: "Other" }]; },
    async getLocalAlbumTracks() { throw new Error("must not be called"); },
    async listLocalDirectory(path) { calls.push(path); return [{ item_type: "file", path: "Music/Legacy Album/01.flac" }]; },
  };
  await assert.rejects(favoriteLocalTracks(favorite, service, true), { message: "SQLITE_LEGACY_FAVORITE_REQUIRES_RESOLUTION" });
  assert.deepEqual(calls, []);
});

test("SQLite mode does not trust a legacy favorite ID collision without its opaque album locator", async () => {
  const favorite = { id: "coincident-id", source: "local", title: "Old title", artist: "Fixture",
    path_or_key: "Music/Old Album", exists: true };
  const calls = [];
  const service = {
    async getLocalAlbums() { return [{ id: favorite.id, folder_path: "sqlite-album://42/new-album", title: "Different album" }]; },
    async getLocalAlbumTracks() { calls.push("tracks"); return []; },
    async listLocalDirectory(path) { calls.push(path); return []; },
  };
  await assert.rejects(favoriteLocalTracks(favorite, service, true), { message: "SQLITE_LEGACY_FAVORITE_REQUIRES_RESOLUTION" });
  assert.deepEqual(calls, []);
});
