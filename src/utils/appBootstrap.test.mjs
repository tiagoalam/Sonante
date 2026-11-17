import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { initialMediaSource, initialSqliteMediaSource, createMainSectionPreference, MAIN_SECTION_STORAGE_KEY, SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY,
  SQLITE_EXPERIMENTAL_PLEX_LIBRARY_STORAGE_KEY, SQLITE_EXPERIMENTAL_LOCAL_SOURCE_STORAGE_KEY,
  preferredSqlitePlexLibrary, saveSqlitePlexLibraryPreference, readSqliteLocalSourcePreference, saveSqliteLocalSourcePreference } from "./appBootstrap.ts";
import { PlayerNavigationRequests, visitPlayerTarget } from "./playerNavigation.ts";

const config = { first_run: false, plex_token: "fixture-token" };
function preference(saved, storageKey = MAIN_SECTION_STORAGE_KEY, initial = initialMediaSource) {
  const published = [], writes = [];
  let reads = 0;
  const controller = createMainSectionPreference(value => published.push(value), () => ({
    getItem(key) { assert.equal(key, storageKey); reads++; return saved; },
    setItem(key, value) { writes.push([key, value]); saved = value; },
  }), storageKey, initial);
  return { controller, published, writes, reads: () => reads };
}

test("Local Browser has no source before configuration resolves", () => {
  assert.equal(initialMediaSource(null), null);
  assert.equal(initialMediaSource({ plex_token: "" }), "local");
  assert.equal(initialMediaSource({ plex_token: " token " }), "plex");
});

test("App gates LocalBrowserView behind admitted initial config", () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.ok(source.indexOf("if (!config || mediaSource === null)") < source.indexOf("<LocalBrowserView"));
});

test("each whitelisted section restores only after config bootstrap without writing", () => {
  for (const section of ["search", "local", "plex", "favorites", "playlists", "explore"]) {
    const state = preference(section);
    assert.deepEqual(state.published, []);
    assert.equal(state.reads(), 0);
    state.controller.restore(null);
    assert.equal(state.published.at(-1), null);
    assert.equal(state.reads(), 0);
    state.controller.restore(config);
    assert.equal(state.published.at(-1), section === "explore" ? "local" : section);
    assert.deepEqual(state.writes, []);
  }
});

test("invalid or absent preference preserves the existing initial source", () => {
  for (const saved of [null, undefined, "", "LOCAL", "settings", "albums", " local", "plex ", "__proto__", '{"section":"local"}', 1, {}]) {
    for (const plex_token of ["", "  ", "fixture-token"]) {
      const state = preference(saved);
      state.controller.restore({ ...config, plex_token });
      assert.equal(state.published.at(-1), initialMediaSource({ plex_token }));
      assert.deepEqual(state.writes, []);
    }
  }
});

test("legacy Equalizer preference restores the independent Equalizer page and remains navigable", () => {
  const state = preference("equalizer");
  state.controller.restore({ ...config, usage_mode: "full" });
  assert.equal(state.published.at(-1), "equalizer");
  assert.deepEqual(state.writes, []);

  state.controller.navigate("equalizer");
  assert.equal(state.published.at(-1), "equalizer");
  assert.deepEqual(state.writes, [[MAIN_SECTION_STORAGE_KEY, "equalizer"]]);
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /setSettingsInitialPage\("audio"\); setShowSettings\(true\)/);
  assert.match(app, /label=\{t\("equalizer\.title"\)\}/);
  assert.doesNotMatch(app, /label=\{t\("settings\.pages\.audio"\)\}/);
});

test("first run ignores storage and wizard completion keeps its existing initial choice", () => {
  for (const saved of ["search", "local", "plex", "favorites", "playlists", "explore"]) {
    for (const plex_token of ["", "fixture-token"]) {
      const state = preference(saved);
      state.controller.navigate("favorites"); // No effective navigation before bootstrap.
      assert.deepEqual(state.published, []);
      state.controller.restore({ ...config, first_run: true, plex_token });
      assert.equal(state.reads(), 0);
      state.controller.navigate("playlists"); // Wizard still owns the screen.
      state.controller.finishSetup({ ...config, plex_token });
      assert.equal(state.published.at(-1), initialMediaSource({ plex_token }));
      assert.deepEqual(state.writes, []);
      state.controller.navigate("favorites");
      assert.deepEqual(state.writes, [[MAIN_SECTION_STORAGE_KEY, "favorites"]]);
    }
  }
});

test("saved Plex falls back to Local without a token or if discovery fails", () => {
  for (const plex_token of ["", "  "]) {
    const state = preference("plex");
    state.controller.restore({ ...config, plex_token });
    assert.equal(state.published.at(-1), "local");
    assert.deepEqual(state.writes, []);
  }
  for (const available of [false, true]) {
    const state = preference("plex");
    state.controller.restore(config);
    state.controller.plexDiscoveryFinished(available);
    assert.equal(state.published.at(-1), available ? "plex" : "local");
    assert.deepEqual(state.writes, []);
  }
  // No preference: preserve the previous token-based bootstrap behavior.
  const legacy = preference(null);
  legacy.controller.restore(config);
  legacy.controller.plexDiscoveryFinished(false);
  assert.equal(legacy.published.at(-1), "plex");
});

test("explicit navigation stores only section names and prevents late restore fallback", () => {
  const state = preference("plex");
  state.controller.restore(config);
  for (const section of ["search", "local", "plex", "favorites", "playlists", "explore"]) {
    state.controller.navigate(section);
    assert.equal(state.published.at(-1), section);
    assert.deepEqual(state.writes.at(-1), [MAIN_SECTION_STORAGE_KEY, section === "explore" ? "local" : section]);
  }
  state.controller.plexDiscoveryFinished(false);
  assert.equal(state.published.at(-1), "explore");
  const count = state.writes.length;
  for (const invalid of ["settings", "__proto__", {}, null]) state.controller.navigate(invalid);
  assert.equal(state.writes.length, count);
  state.controller.restore(config); // Remount/restart reads the last effective navigation.
  assert.equal(state.published.at(-1), "local");
  assert.equal(state.writes.length, count);
});

test("Storage absent or throwing never prevents bootstrap or navigation", () => {
  for (const storage of [() => null, () => { throw Error("denied"); },
    () => ({ getItem() { throw Error("read denied"); }, setItem() { throw Error("quota"); } })]) {
    const published = [];
    const controller = createMainSectionPreference(value => published.push(value), storage);
    assert.doesNotThrow(() => controller.restore(config));
    assert.equal(published.at(-1), "plex");
    assert.doesNotThrow(() => controller.navigate("favorites"));
    assert.equal(published.at(-1), "favorites");
  }
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  try {
    Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw Error("denied"); } });
    const published = [];
    const controller = createMainSectionPreference(value => published.push(value));
    controller.restore(config);
    controller.navigate("local");
    assert.deepEqual(published, ["plex", "local"]);
  } finally {
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else delete globalThis.localStorage;
  }
});

test("App restores the saved experimental section without replacing it during bootstrap", () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const bootstrap = source.slice(source.indexOf("// Carga inicial de configurações"), source.indexOf("NOW_PLAYING_READY_EVENT", source.indexOf("// Carga inicial de configurações")));
  assert.match(bootstrap, /if \(disposed\) return;[\s\S]*?setConfig\(cfg\);[\s\S]*?mainSectionPreference\.restore\(cfg\)/);
  assert.doesNotMatch(bootstrap, /\.navigate\(|finishSetup\(cfg\)/);
  assert.match(source, /createMainSectionPreference\(setMediaSource, undefined,[\s\S]*sqliteMode \? SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY : MAIN_SECTION_STORAGE_KEY,[\s\S]*sqliteMode \? initialSqliteMediaSource : initialMediaSource/);
  assert.match(source, /mainSectionPreference\.finishSetup\(newConfig\)/);
  assert.match(source, /preferredSqlitePlexLibrary\(libs, config\?\.plex_server_id\)/);
  assert.match(source, /saveSqlitePlexLibraryPreference\(config\?\.plex_server_id, library\.key\)/);
  assert.match(source, /if \(config && config\.first_run\)[\s\S]*?onFinish=[\s\S]*?mainSectionPreference\.finishSetup\(newConfig\)/);
  const sidebar = source.slice(source.indexOf("{/* Barra Lateral */}"), source.indexOf("{/* Painel Central */}"));
  for (const section of ["search", "local", "plex", "favorites", "playlists", "explore"]) assert.ok(sidebar.includes(section === "local" ? 'navigateLocalSubview("browser")' : section === "explore" ? "navigateExplore()" : `mainSectionPreference.navigate("${section}")`));
  assert.ok(sidebar.includes('label={t("equalizer.title")}'));
  assert.doesNotMatch(sidebar, /label=\{t\("settings\.pages\.audio"\)\}/);
  const player = source.slice(source.indexOf("<PlayerBar"), source.indexOf("{(configSavePhase || configSaveNotice)"));
  assert.ok(player.includes('onNavigateToArtist={playerSourceKind === "upnp" ? undefined : () => navigateFromPlayer("artist")}'));
  assert.ok(player.includes('onNavigateToAlbum={playerSourceKind === "upnp" ? undefined : () => navigateFromPlayer("album")}'));
  assert.ok(source.includes("mainSectionPreference.navigate(target.kind)"));
  assert.doesNotMatch(source, /setMediaSource\(/); // All state writes belong to the controller.
  assert.match(source, /mainSectionPreference\.plexDiscoveryFinished\(libs\.length > 0\)/);
  assert.match(source, /if \(!disposed\) \{\s*mainSectionPreference\.plexDiscoveryFinished\(false\)/);
});

test("SQLite profile persists only its own screen preference and async discovery cannot overwrite Local", () => {
  const state = preference("local", SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY, initialSqliteMediaSource);
  state.controller.restore(config);
  assert.equal(state.published.at(-1), "local");
  for (const available of [true, false]) {
    state.controller.plexDiscoveryFinished(available);
    assert.equal(state.published.at(-1), "local");
  }
  state.controller.navigate("plex");
  assert.deepEqual(state.writes, [[SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY, "plex"]]);
});

test("SQLite profile defaults to Local, restores an available saved Plex screen and falls back safely", () => {
  const noPreference = preference(null, SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY, initialSqliteMediaSource);
  noPreference.controller.restore(config);
  assert.equal(noPreference.published.at(-1), "local");
  const savedPlex = preference("plex", SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY, initialSqliteMediaSource);
  savedPlex.controller.restore(config);
  assert.equal(savedPlex.published.at(-1), "plex");
  savedPlex.controller.plexDiscoveryFinished(true);
  assert.equal(savedPlex.published.at(-1), "plex");
  const unavailablePlex = preference("plex", SQLITE_EXPERIMENTAL_MAIN_SECTION_STORAGE_KEY, initialSqliteMediaSource);
  unavailablePlex.controller.restore({ ...config, plex_token: "" });
  assert.equal(unavailablePlex.published.at(-1), "local");
});

test("SQLite Plex library preference restores only a library still available on the same server", () => {
  let stored = null;
  const storage = () => ({
    getItem(key) { assert.equal(key, SQLITE_EXPERIMENTAL_PLEX_LIBRARY_STORAGE_KEY); return stored; },
    setItem(key, value) { assert.equal(key, SQLITE_EXPERIMENTAL_PLEX_LIBRARY_STORAGE_KEY); stored = value; },
  });
  const libraries = [{ key: "music-a", title: "Music A" }, { key: "music-b", title: "Music B" }];
  assert.equal(preferredSqlitePlexLibrary(libraries, "server-1", storage).key, "music-a");
  saveSqlitePlexLibraryPreference("server-1", "music-b", storage);
  assert.equal(preferredSqlitePlexLibrary(libraries, "server-1", storage).key, "music-b");
  assert.equal(preferredSqlitePlexLibrary(libraries, "server-2", storage).key, "music-a");
  assert.equal(preferredSqlitePlexLibrary(libraries.slice(0, 1), "server-1", storage).key, "music-a");
  assert.equal(preferredSqlitePlexLibrary([], "server-1", storage), null);
});

test("SQLite Local source selection is persisted in its own profile key and empty selection is safe", () => {
  let stored = null;
  const storage = () => ({
    getItem(key) { assert.equal(key, SQLITE_EXPERIMENTAL_LOCAL_SOURCE_STORAGE_KEY); return stored; },
    setItem(key, value) { assert.equal(key, SQLITE_EXPERIMENTAL_LOCAL_SOURCE_STORAGE_KEY); stored = value; },
  });
  assert.equal(readSqliteLocalSourcePreference(storage), null);
  saveSqliteLocalSourcePreference("sqlite-2", storage);
  assert.equal(readSqliteLocalSourcePreference(storage), "sqlite-2");
  saveSqliteLocalSourcePreference(null, storage);
  assert.equal(readSqliteLocalSourcePreference(storage), null);
});

test("actual guarded player handler persists only a resolved destination, not contextual data", async () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const body = source.match(/const navigateFromPlayer = \(view: "album" \| "artist"\) => \{([\s\S]*?)\n  \};/)?.[1];
  assert.ok(body);
  for (const [kind, view, expected] of [["local", "artist", "local"], ["plex", "artist", "plex"],
    ["local", "album", "local"], ["plex", "album", "plex"], ["navidrome", "album", "navidrome"], ["navidrome", "artist", "navidrome"], [null, "album", null]]) {
    const state = preference("playlists");
    state.controller.restore(config);
    const media = kind === "local" ? { kind, uri: "Source/Album/one.flac", queue_index: 0 }
      : kind === "plex" ? { kind, server_id: "server", part_key: "/library/parts/1/file.flac", queue_index: 0 } : kind === "navidrome" ? {kind,connection_id:"session",song_id:"song",queue_index:0} : null;
    let session = null;
    const handler = new Function("playbackStatus", "mainSectionPreference", "playerNavigationRequests", "localCatalog", "plexService", "audioService", "navidromeService",
      "setPlayerNavigationError", "setPlayerNavigation", "visitPlayerTarget", "centralSection", "sqliteMode", "sqliteLibrary", `return view => {${body}};`)(
      { current_media: media, artist: "Artist" }, state.controller, new PlayerNavigationRequests(), {
        loaded: true, albums: [{ id: "one", folder_path: "Source/Album", source_id: "Source", artist: "Artist", discs: [] }],
      }, { resolvePlayerNavigation: async input => ({ media: input, navigation: {
        album: { rating_key: "album-1", artist_rating_key: "artist-1" }, artist_rating_key: "artist-1", artist_name: "Artist",
      } }) }, { getLocalAlbumTracks: async () => [{ tracks: [{ item_type: "file", path: "Source/Album/one.flac" }] }] }, {resolvePlayerNavigation:async media=>({...media,account_id:"a".repeat(64),album_id:"album",artist_id:"artist"})},
      () => {}, update => { session = update(session); }, visitPlayerTarget, "playlists", false, { albumForTrack: async () => null });
    handler(view);
    for (let i = 0; i < 4; i++) await Promise.resolve();
    assert.equal(state.published.at(-1), expected ?? "playlists");
    assert.deepEqual(state.writes, expected ? [[MAIN_SECTION_STORAGE_KEY, expected]] : []);
    if (expected) assert.equal(session.returnSection, "playlists");
  }
});

test("actual player handler routes an opaque SQLite now-playing locator through its exact catalog association", async () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const body = source.match(/const navigateFromPlayer = \(view: "album" \| "artist"\) => \{([\s\S]*?)\n  \};/)?.[1];
  assert.ok(body);
  const state = preference("local"); state.controller.restore(config);
  const locator = "sqlite-track://17/QWxidW0vMDMud2F2";
  const albumId = "sqlite:17:YWxidW0";
  let session = null, navigationError = null; const calls = [];
  const handler = new Function("playbackStatus", "mainSectionPreference", "playerNavigationRequests", "localCatalog", "plexService", "audioService", "navidromeService",
    "setPlayerNavigationError", "setPlayerNavigation", "visitPlayerTarget", "centralSection", "sqliteMode", "sqliteLibrary", `return view => {${body}};`)(
    { current_media: { kind: "local", uri: locator, queue_index: 0 }, artist: "Guest Artist" }, state.controller,
    new PlayerNavigationRequests(), { loaded: true, albums: [{ id: albumId, folder_path: "sqlite-album://" + albumId,
      source_id: "sqlite-17", artist: "Various Artists", artist_names: ["Guest Artist"], discs: [] }] },
    { resolvePlayerNavigation: async () => { throw Error("Plex must not be queried"); } },
    { getLocalAlbumTracks: async id => { calls.push(["tracks", id]); return [{ disc: { number: 1 }, tracks: [{ item_type: "file", path: "opaque-track" }] }]; } },
    { resolvePlayerNavigation: async () => { throw Error("Navidrome must not be queried"); } },
    value => { navigationError = value; }, update => { session = update(session); }, visitPlayerTarget, "local", true,
    { albumForTrack: async value => { calls.push(["resolve", value]); return albumId; } });
  handler("artist");
  for (let i = 0; i < 8; i++) await Promise.resolve();
  assert.deepEqual(calls, [["resolve", locator], ["tracks", albumId]]);
  assert.equal(session.target.kind, "local");
  assert.equal(session.target.albumId, albumId);
  assert.equal(session.target.artistName, "Guest Artist");
  assert.equal(navigationError, false);
  assert.equal(state.published.at(-1), "local");
});


test("listening sessions never replace the principal startup destination", () => {
  const state = preference("local"); state.controller.restore(config);
  state.controller.navigate("sessions"); assert.equal(state.published.at(-1), "sessions");
  assert.deepEqual(state.writes, []);
  state.controller.restore(config); assert.equal(state.published.at(-1), "local");
  const legacy = preference("sessions"); legacy.controller.restore(config);
  assert.equal(legacy.published.at(-1), "plex");
});

test("album editions are explicit navigation and never replace the principal startup destination", () => {
  const state = preference("local"); state.controller.restore(config);
  state.controller.navigate("editions"); assert.equal(state.published.at(-1), "editions");
  assert.deepEqual(state.writes, []);
  state.controller.restore(config); assert.equal(state.published.at(-1), "local");
  const legacy = preference("editions"); legacy.controller.restore(config);
  assert.equal(legacy.published.at(-1), "plex");
});
