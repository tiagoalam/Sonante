import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { localAlbumForMedia, currentMediaKey, PlayerNavigationRequests, visitPlayerTarget, createPlayerAlbumLookup } from "./playerNavigation.ts";

const local = (uri, queue_index = 0) => ({ kind: "local", uri, queue_index });
const plex = (server_id = "server-a", part_key = "/library/parts/1/file.flac", queue_index = 0) => ({ kind: "plex", server_id, part_key, queue_index });
const album = (id, folder_path, source_id = "Source", discs = [folder_path]) => ({
  id, folder_path, source_id, title: "Same album", artist: "Same artist", track_count: 2,
  discs: discs.map((path, i) => ({ folder_path: path, label: `Disc ${i + 1}`, number: i + 1, track_count: 1 })),
});
const answer = (media = plex(), key = "album-1", artist = "artist-1") => ({
  media, navigation: {
    album: { rating_key: key, title: "Same album", artist: "Same artist", artist_rating_key: artist },
    artist_rating_key: artist, artist_name: "Same artist",
  },
});
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const groupsFor = uri => [{ disc: { folder_path: uri.slice(0, uri.lastIndexOf("/")), number: 1, label: "Disc", track_count: 1 },
  tracks: [{ item_type: "file", path: uri, name: "one.flac" }] }];
function fixture() {
  const requests = new PlayerNavigationRequests(), publications = [], failures = [], calls = [], localCalls = [];
  requests.setContext("initial");
  const open = (media = plex(), view = "album", albums = [], lookupLocal = id => { localCalls.push(id); return Promise.resolve(groupsFor(media.uri)); }) => requests.open(media, view, albums,
    input => { const pending = deferred(); calls.push({ input, ...pending }); return pending.promise; },
    target => publications.push(target), () => failures.push(true), undefined, lookupLocal);
  return { requests, open, publications, failures, calls, localCalls };
}

test("Local album navigation is exact by directory and source, never by homonymous tags", () => {
  const albums = [album("a", "Source A/Álbum com espaços", "Source A"), album("b", "Source B/Álbum com espaços", "Source B")];
  assert.equal(localAlbumForMedia(local("Source B/Álbum com espaços/03.flac"), albums)?.id, "b");
  assert.equal(localAlbumForMedia(local("Source A/Álbum com espaços/01.flac"), albums)?.id, "a");
  assert.equal(localAlbumForMedia(local("Source B/Álbum com espaços extra/01.flac"), albums), null);
});

test("disc paths identify multidisc and CUE albums without physical-folder heuristics", () => {
  const albums = [album("multi", "Source/Box", "Source", ["Source/Box/CD1", "Source/Box/CD2/Imagem.CUE"]),
    album("single-cue", "Source/Single", "Source", ["Source/Single/Imagem.cue"]),
    album("physical-wins", "Source/Physical")];
  for (const [uri, id] of [["Source/Box/CD1/one.flac", "multi"], ["Source/Box/CD2/Imagem.CUE/track0002", "multi"],
    ["Source/Single/Imagem.cue/track0001", "single-cue"]]) {
    assert.equal(localAlbumForMedia(local(uri), albums)?.id, id);
  }
  for (const uri of ["Source/Physical/Imagem.cue/track0001", "Source/Single/image.flac", "Source/Box/unknown/one.flac"]) {
    assert.equal(localAlbumForMedia(local(uri), albums), null);
  }
});

test("ambiguous album groups, missing identity and invalid URIs fail closed", () => {
  const albums = [album("one", "Source/Folder"), album("two", "Source/Folder")];
  assert.equal(localAlbumForMedia(local("Source/Folder/a.flac"), albums), null);
  for (const media of [null, plex(), local("/Source/Folder/a.flac"), local("Source/../Folder/a.flac"),
    local("Source//Folder/a.flac"), local("https://server/file"), local("Source/Folder/a\n.flac")]) {
    assert.equal(localAlbumForMedia(media, albums), null);
  }
});

test("Local album and artist visits use the same proven catalogue identity, without Plex I/O", async () => {
  const albums = [album("one", "Source/Folder")];
  for (const view of ["album", "artist"]) {
    const f = fixture();
    await f.open(local("Source/Folder/a.flac"), view, albums);
    assert.equal(f.publications.length, 1);
    assert.deepEqual(f.publications[0], { kind: "local", view, albumId: "one", verified: { groups: groupsFor("Source/Folder/a.flac"), catalog: albums } });
    assert.equal(f.publications[0].verified.catalog, albums);
    assert.deepEqual(f.localCalls, ["one"]);
    assert.equal(f.calls.length, 0);
    assert.equal(f.failures.length, 0);
  }
  const f = fixture();
  await f.open(local("Legacy/Folder/a.flac"), "artist", [album("legacy", "Legacy/Folder", null)]);
  assert.equal(f.publications.length, 0); // No unproven cross-source artist scope.
  assert.equal(f.failures.length, 1);
});

test("a track artist differing from the anchored Local album artist cannot open another discography", async () => {
  const requests = new PlayerNavigationRequests(), targets = [], failures = [];
  await requests.open(local("Source/Compilation/one.flac"), "artist", [album("one", "Source/Compilation")],
    () => { throw Error("unexpected Plex request"); }, target => targets.push(target), () => failures.push(true), "Another artist",
    () => { throw Error("unexpected Local request"); });
  assert.equal(targets.length, 0); assert.equal(failures.length, 1);
});

test("a stale URI in a still existing directory cannot select the remaining album", async () => {
  const f = fixture();
  await f.open(local("Source/Folder/removed.flac"), "album", [album("remaining", "Source/Folder")],
    async () => groupsFor("Source/Folder/present.flac"));
  assert.equal(f.publications.length, 0); assert.equal(f.failures.length, 1);
});

test("Local verification is invalidated by catalogue/navigation changes and read failures", async () => {
  for (const stale of [false, true]) {
    const f = fixture(), response = deferred();
    const pending = f.open(local("Source/Folder/one.flac"), "album", [album("one", "Source/Folder")], () => response.promise);
    if (stale) f.requests.setContext("catalogue replaced");
    response.reject(Error("read failed")); await pending;
    assert.equal(f.publications.length, 0); assert.equal(f.failures.length, stale ? 0 : 1);
  }
  const f = fixture(), response = deferred();
  const pending = f.open(local("Source/Folder/one.flac"), "album", [album("one", "Source/Folder")], () => response.promise);
  f.requests.invalidate(); response.resolve(groupsFor("Source/Folder/one.flac")); await pending;
  assert.equal(f.publications.length, 0);
});

test("virtual CUE navigation verifies the exact URI and preserves the proved groups for the destination", async () => {
  const uri = "Source/Álbum/Imagem.CUE/track0002", albums = [album("cue", "Source/Álbum", "Source", ["Source/Álbum/Imagem.CUE"])];
  const f = fixture();
  await f.open(local(uri), "album", albums);
  assert.equal(f.publications[0].albumId, "cue");
  assert.deepEqual(f.publications[0].verified.groups, groupsFor(uri));
  assert.equal(f.localCalls.length, 1);
});

test("verified Local tracks are reused without I/O, including effect replay, only for the same catalogue", async () => {
  const original = [album("one", "Source/Album")], groups = groupsFor("Source/Album/one.flac"), calls = [];
  let catalog = original;
  const target = { kind: "local", view: "album", albumId: "one", verified: { groups, catalog } };
  const load = createPlayerAlbumLookup(target, () => catalog, async id => { calls.push(id); return []; });
  assert.equal(await load("one"), groups); assert.equal(await load("one"), groups);
  assert.equal(calls.length, 0);
  await load("other"); assert.deepEqual(calls, ["other"]);
  catalog = [...original]; await load("one"); assert.deepEqual(calls, ["other", "one"]);
  catalog = original; await load("one"); assert.deepEqual(calls, ["other", "one", "one"]);
});

test("switching between Plex and Local cannot publish the previous source's delayed result", async () => {
  const f = fixture(), pending = f.open(plex());
  await f.open(local("Source/Album/one.flac"), "album", [album("local", "Source/Album")]);
  f.calls[0].resolve(answer()); await pending;
  assert.equal(f.publications.length, 1); assert.equal(f.publications[0].kind, "local");
  const next = f.open(plex("server-b"));
  f.calls[1].resolve(answer(plex("server-b"), "another-album", "another-artist")); await next;
  assert.equal(f.publications.at(-1).kind, "plex");
  assert.equal(f.publications.at(-1).album.rating_key, "another-album");
});

test("Plex navigates using returned album/artist IDs even when titles are identical", async () => {
  const f = fixture();
  for (const [view, key, artist] of [["album", "album-a", "artist-a"], ["artist", "album-b", "artist-b"]]) {
    const pending = f.open(plex(), view);
    f.calls.at(-1).resolve(answer(plex(), key, artist)); await pending;
    assert.equal(f.publications.at(-1).album.rating_key, key);
    assert.equal(f.publications.at(-1).artist.rating_key, artist);
    assert.equal(f.publications.at(-1).view, view);
  }
});

test("Plex mismatched server, part, occurrence and incomplete responses preserve the current view", async () => {
  for (const response of [answer(plex("server-b")), answer(plex("server-a", "/other-part")), answer(plex("server-a", undefined, 1)),
    { media: plex(), navigation: {} }, { ...answer(), navigation: { ...answer().navigation, artist_rating_key: "other-artist" } }]) {
    const f = fixture(), pending = f.open();
    f.calls[0].resolve(response); await pending;
    assert.deepEqual(f.publications, []); assert.equal(f.failures.length, 1);
  }
});

test("track/source/navigation changes and disposal invalidate successful and failed responses", async () => {
  for (const change of ["new-track", "new-source", "new-navigation", "dispose"]) {
    for (const error of [false, true]) {
      const f = fixture(), pending = f.open();
      if (change === "dispose") f.requests.invalidate(); else f.requests.setContext(change);
      if (error) f.calls[0].reject(new Error("failed")); else f.calls[0].resolve(answer());
      await pending;
      assert.deepEqual(f.publications, []); assert.deepEqual(f.failures, []);
    }
  }
});

test("a newer click supersedes the previous request; unchanged telemetry does not invalidate it", async () => {
  const f = fixture(), previous = f.open(), latest = f.open(plex(), "artist");
  f.requests.setContext("initial");
  f.calls[1].resolve(answer()); await latest;
  f.calls[0].resolve(answer(plex(), "old-album")); await previous;
  assert.equal(f.publications.length, 1); assert.equal(f.publications[0].view, "artist");
  assert.notEqual(currentMediaKey(local("Source/one.flac")), currentMediaKey(plex()));
  assert.equal(currentMediaKey({ ...plex() }), currentMediaKey(plex()));
});

test("lookup failure and absent current media show failure without navigating or textual fallback", async () => {
  const f = fixture(), pending = f.open();
  f.calls[0].reject(new Error("read failed")); await pending;
  await f.open(null);
  assert.equal(f.publications.length, 0); assert.equal(f.failures.length, 2);
});

test("successive Local/Plex visits retain the original return section, including contextual views", () => {
  for (const section of ["local", "plex", "favorites", "playlists"]) {
    let session = visitPlayerTarget(null, { kind: "local", view: "album", albumId: "one", verified: { groups: [], catalog: [] } }, section);
    const target = { kind: "plex", view: "artist", album: answer().navigation.album, artist: { rating_key: "artist-1", name: "Same artist" } };
    session = visitPlayerTarget(session, target, "local");
    assert.equal(session.revision, 2);
    assert.equal(session.returnSection, section);
    assert.equal(session.target, target);
    assert.equal(visitPlayerTarget(null, target, "plex").returnSection, "plex");
  }
});

test("IPC is read-only and player links use guarded identity resolution with a mounted return view", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const browser = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  const service = readFileSync(new URL("../services/plex.ts", import.meta.url), "utf8");
  assert.ok(app.includes('onNavigateToAlbum={() => navigateFromPlayer("album")}'));
  assert.ok(app.includes('onNavigateToArtist={() => navigateFromPlayer("artist")}'));
  assert.ok(app.includes('className={playerTarget ? "hidden" : "contents"}'));
  assert.ok(app.includes("const centralSection = playerReturnSection ?? mediaSource"));
  assert.match(app, /else if \(playerTarget\) \{\s*closePlayerNavigation\(\)/);
  assert.match(app, /playerNavigationRequests\.invalidate\(\);\s*audioService\.next\(\)/);
  assert.match(app, /playerNavigationRequests\.invalidate\(\);\s*audioService\.previous\(\)/);
  assert.ok(app.includes("onClickCapture={() => playerNavigationRequests.invalidate()}"));
  assert.ok(browser.includes("albums.find(item => item.id === playerTarget.albumId)"));
  assert.ok(browser.includes("if (!playerTarget) localAlbumCatalog.setUpdating(isLibraryUpdating, true)"));
  assert.ok(service.includes('invoke<PlayerPlexNavigation>("resolve_player_plex_navigation", { media })'));
});
