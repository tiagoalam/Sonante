import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { initialMediaSource, createMainSectionPreference, MAIN_SECTION_STORAGE_KEY } from "./appBootstrap.ts";
import { PlayerNavigationRequests, visitPlayerTarget } from "./playerNavigation.ts";

const config = { first_run: false, plex_token: "fixture-token" };
function preference(saved) {
  const published = [], writes = [];
  let reads = 0;
  const controller = createMainSectionPreference(value => published.push(value), () => ({
    getItem(key) { assert.equal(key, MAIN_SECTION_STORAGE_KEY); reads++; return saved; },
    setItem(key, value) { writes.push([key, value]); saved = value; },
  }));
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

test("App persists sidebar and contextual section navigation, never bootstrap or wizard", () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const bootstrap = source.slice(source.indexOf("// Carga inicial de configurações"), source.indexOf("NOW_PLAYING_READY_EVENT", source.indexOf("// Carga inicial de configurações")));
  assert.match(bootstrap, /if \(disposed\) return;[\s\S]*?setConfig\(cfg\);[\s\S]*?mainSectionPreference\.restore\(cfg\)/);
  assert.doesNotMatch(bootstrap, /\.navigate\(/);
  assert.match(source, /if \(config && config\.first_run\)[\s\S]*?onFinish=[\s\S]*?mainSectionPreference\.finishSetup\(newConfig\)/);
  const sidebar = source.slice(source.indexOf("{/* Barra Lateral */}"), source.indexOf("{/* Painel Central */}"));
  for (const section of ["search", "local", "plex", "favorites", "playlists", "explore"]) assert.ok(sidebar.includes(section === "local" ? 'navigateLocalSubview("browser")' : section === "explore" ? "navigateExplore()" : `mainSectionPreference.navigate("${section}")`));
  const player = source.slice(source.indexOf("<PlayerBar"), source.indexOf("{(configSavePhase || configSaveNotice)"));
  assert.ok(player.includes('onNavigateToArtist={playerSourceKind === "upnp" ? undefined : () => navigateFromPlayer("artist")}'));
  assert.ok(player.includes('onNavigateToAlbum={playerSourceKind === "upnp" ? undefined : () => navigateFromPlayer("album")}'));
  assert.ok(source.includes("mainSectionPreference.navigate(target.kind)"));
  assert.doesNotMatch(source, /setMediaSource\(/); // All state writes belong to the controller.
  assert.match(source, /mainSectionPreference\.plexDiscoveryFinished\(libs\.length > 0\)/);
  assert.match(source, /if \(!disposed\) \{\s*mainSectionPreference\.plexDiscoveryFinished\(false\)/);
});

test("actual guarded player handler persists only a resolved destination, not contextual data", async () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const body = source.match(/const navigateFromPlayer = \(view: "album" \| "artist"\) => \{([\s\S]*?)\n  \};/)?.[1];
  assert.ok(body);
  for (const [kind, view, expected] of [["local", "artist", "local"], ["plex", "artist", "plex"],
    ["local", "album", "local"], ["plex", "album", "plex"], [null, "album", null]]) {
    const state = preference("playlists");
    state.controller.restore(config);
    const media = kind === "local" ? { kind, uri: "Source/Album/one.flac", queue_index: 0 }
      : kind === "plex" ? { kind, server_id: "server", part_key: "/library/parts/1/file.flac", queue_index: 0 } : null;
    let session = null;
    const handler = new Function("playbackStatus", "mainSectionPreference", "playerNavigationRequests", "localCatalog", "plexService", "audioService",
      "setPlayerNavigationError", "setPlayerNavigation", "visitPlayerTarget", "centralSection", `return view => {${body}};`)(
      { current_media: media, artist: "Artist" }, state.controller, new PlayerNavigationRequests(), {
        loaded: true, albums: [{ id: "one", folder_path: "Source/Album", source_id: "Source", artist: "Artist", discs: [] }],
      }, { resolvePlayerNavigation: async input => ({ media: input, navigation: {
        album: { rating_key: "album-1", artist_rating_key: "artist-1" }, artist_rating_key: "artist-1", artist_name: "Artist",
      } }) }, { getLocalAlbumTracks: async () => [{ tracks: [{ item_type: "file", path: "Source/Album/one.flac" }] }] },
      () => {}, update => { session = update(session); }, visitPlayerTarget, "playlists");
    handler(view);
    for (let i = 0; i < 4; i++) await Promise.resolve();
    assert.equal(state.published.at(-1), expected ?? "playlists");
    assert.deepEqual(state.writes, expected ? [[MAIN_SECTION_STORAGE_KEY, expected]] : []);
    if (expected) assert.equal(session.returnSection, "playlists");
  }
});
