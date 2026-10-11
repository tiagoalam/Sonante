import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import i18next from "i18next";
import { configuredSignalRoute, parseReportedMpdFormat, signalPathInformation } from "./signalPath.ts";

const config = { first_run: false, audio_output_type: "alsa", alsa_device: "hw:CARD=Fixture,DEV=0", plex_token: "secret-token", local_folders: ["/home/private/music"] };
const status = { state: "play", audio_format: "96000:24:2", current_media: { kind: "local", uri: "private.cue/track0001" }, volume: { backend: "mpd_software" } };
const health = { state: "available" };
const resource = language => JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url)));
function signalComponent(t) {
  const source = readFileSync(new URL("../components/SignalPath.tsx", import.meta.url), "utf8");
  const js = ts.transpileModule(source, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  const dependencies = {
    react: { ...React, default: React }, "react/jsx-runtime": jsx, "lucide-react": icons,
    "react-i18next": { useTranslation: () => ({ t }) }, "../utils/signalPath": { signalPathInformation },
  };
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, exports);
  return exports.SignalPath;
}

test("confirmed route is minimal and never publishes arbitrary PCM names or configuration secrets", () => {
  assert.deepEqual(configuredSignalRoute(config), { mode: "direct", endpoint: config.alsa_device });
  for (const choice of [{ audio_output_type: "pipewire" }, { alsa_device: "default" }]) {
    assert.deepEqual(configuredSignalRoute({ ...config, ...choice }), { mode: "shared", endpoint: "default" });
  }
  for (const endpoint of ["/home/private/pcm", "https://user:secret@server", "hw:Fixture,0?token=secret", "custom", "hw:../../private,0"]) {
    const route = configuredSignalRoute({ ...config, alsa_device: endpoint });
    assert.equal(route.endpoint, null);
    assert.doesNotMatch(JSON.stringify(route), /private|secret|https/);
  }
  assert.equal(configuredSignalRoute(null), null);
  assert.equal(configuredSignalRoute({ ...config, first_run: true }), null);
  assert.deepEqual(Object.keys(configuredSignalRoute(config)), ["mode", "endpoint"]);
});

test("only known decoded PCM structures are parsed, without codec or source quality inference", () => {
  for (const [value, rate, bits] of [["44100:16:2", 44.1,16], ["96000:24:2",96,24], ["192000:24:2",192,24]]) {
    assert.deepEqual(parseReportedMpdFormat(value), { kind: "pcm", rateKhz: rate, bits, channels: 2 });
  }
  assert.deepEqual(parseReportedMpdFormat(""), { kind: "absent" });
  for (const value of ["dsd64:2", "48000:f:2", "FLAC", "0:24:2", "96000:24:0", "96000:24:999", "secret /private"]) {
    assert.deepEqual(parseReportedMpdFormat(value), { kind: "unknown" });
  }
});

test("source comes from current_media, while paused, stopped and unavailable states are explicit", () => {
  for (const kind of ["local", "plex"]) assert.equal(signalPathInformation({ ...status, current_media: { kind } }, health).source, kind);
  assert.equal(signalPathInformation({ ...status, current_media: null }, health).source, null);
  const paused = signalPathInformation({ ...status, state: "pause" }, health);
  assert.equal(paused.state, "paused"); assert.equal(paused.format.kind, "pcm");
  const stopped = signalPathInformation({ ...status, state: "stop" }, health);
  assert.equal(stopped.state, "stopped"); assert.equal(stopped.format.kind, "absent");
  for (const state of ["starting", "unavailable", "stopping"]) {
    const unavailable = signalPathInformation(status, { state });
    assert.deepEqual(unavailable, { state: "unavailable", source: null, format: { kind: "absent" }, volumeBackend: "unavailable" });
  }
});

test("both variants use translated reported metadata and detailed view has compact accessible help", async () => {
  for (const language of ["en", "pt-BR"]) {
    const i18n = i18next.createInstance();
    await i18n.init({ lng: language, resources: { [language]: { translation: resource(language) } }, interpolation: { escapeValue: false } });
    const t = i18n.t.bind(i18n), SignalPath = signalComponent(t);
    for (const mode of ["shared", "direct"]) {
      const route = configuredSignalRoute({ ...config, audio_output_type: mode === "shared" ? "pipewire" : "alsa" });
      const html = renderToStaticMarkup(React.createElement(SignalPath, { route, status, health }));
      assert.ok(html.includes(t(`signalPath.${mode}`)));
      assert.match(html, /96 kHz/); assert.match(html, /24-bit/);
      assert.doesNotMatch(html, /<details|<summary/);
      assert.match(html, /<ol/);
      assert.match(html, /role="tooltip"/);
      assert.match(html, /group-focus-within\/signal-help:block/);
      assert.ok(html.includes(t("signalPath.help")));
      assert.ok(html.includes(mode === "shared" ? "grid-cols-5" : "grid-cols-4"));
      assert.doesNotMatch(html, /flex-col items-center gap-1 rounded-lg|py-2/);
      assert.equal(html.includes(`${t("signalPath.endpointShort")}:`), mode === "direct");
      assert.ok(html.includes(mode === "shared" ? "ALSA default" : "ALSA hw"));
      assert.ok(html.includes(t("signalPath.reported")));
      assert.doesNotMatch(html, /<p[^>]*>Configured route|<p[^>]*>Rota configurada/);
      const noEndpoint = renderToStaticMarkup(React.createElement(SignalPath, { route: { mode: "direct", endpoint: null }, status, health }));
      assert.ok(!noEndpoint.includes(`${t("signalPath.endpointShort")}:`));
      const stopped = renderToStaticMarkup(React.createElement(SignalPath, { route, status: { ...status, state: "stop" }, health }));
      assert.ok(stopped.includes(t("signalPath.stopped"))); assert.doesNotMatch(stopped, /96 kHz|24-bit/);
      assert.doesNotMatch(html, /secret-token|private\.cue|\/home\/private/);
      const compact = renderToStaticMarkup(React.createElement(SignalPath, { route, status, health, compact: true }));
      assert.match(compact, /tabindex="0"/); assert.match(compact, /aria-label=/);
      assert.doesNotMatch(compact, /<button|<input|<details/);
      const unavailable = renderToStaticMarkup(React.createElement(SignalPath, { route, status, health: { state: "unavailable" } }));
      assert.ok(unavailable.includes(t("signalPath.unavailable"))); assert.doesNotMatch(unavailable, /96 kHz|24-bit/);
      assert.deepEqual(Object.keys(resource("en").signalPath).sort(), Object.keys(resource("pt-BR").signalPath).sort());
    }
  }
});

test("snapshot route follows confirmed config and shared presentation leaves playback controls intact", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const snapshotEffect = app.slice(app.indexOf("nowPlayingSnapshotRef.current = {"), app.indexOf("// Atualiza favoritos Plex"));
  assert.match(snapshotEffect, /route: configuredSignalRoute\(config\)/);
  assert.match(snapshotEffect, /\[config, mpdHealth, playbackStatus,/);
  assert.doesNotMatch(snapshotEffect, /settingsDraft|setInterval|setTimeout|invoke/);
  const before = { route: configuredSignalRoute(config), playback: status, health };
  const draft = { ...config, audio_output_type: "pipewire" };
  assert.equal(before.route.mode, "direct");
  const after = { ...before, route: configuredSignalRoute(draft) };
  assert.equal(JSON.parse(JSON.stringify(after)).route.mode, "shared");
  const window = readFileSync(new URL("../components/NowPlayingWindow.tsx", import.meta.url), "utf8");
  assert.match(window, /listen<NowPlayingSnapshot>\(NOW_PLAYING_SNAPSHOT_EVENT/);
  assert.match(window, /<SignalPath route=\{snapshot\?\.route \?\? null\}/);
  const player = readFileSync(new URL("../components/PlayerBar.tsx", import.meta.url), "utf8");
  assert.match(player, /<footer className="h-28 /);
  for (const callback of ["handlePlayToggle", "handleSeekCommit", "handleNext", "handlePrevious", "handleVolumeChange", "onToggleQueue", "onNavigateToAlbum", "onNavigateToArtist"]) assert.ok(player.includes(callback));
  const component = readFileSync(new URL("../components/SignalPath.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(component, /audioService|invoke\(|setInterval|setTimeout/);
});

test("Now Playing receives route changes through the existing snapshot event and never commands playback", async () => {
  const i18n = i18next.createInstance();
  await i18n.init({ lng: "en", resources: { en: { translation: resource("en") } }, interpolation: { escapeValue: false } });
  const t = i18n.t.bind(i18n), SignalPath = signalComponent(t);
  let index = 0;
  const slots = [], effects = [], listeners = {}, playbackCalls = [];
  const hooks = {
    ...React,
    useState(initial) { const key = index++; if (!(key in slots)) slots[key] = initial; return [slots[key], value => { slots[key] = value; }]; },
    useRef(initial) { const key = index++; return slots[key] ??= { current: initial }; },
    useEffect(effect) { effects.push(effect); },
  };
  const source = readFileSync(new URL("../components/NowPlayingWindow.tsx", import.meta.url), "utf8");
  const js = ts.transpileModule(source, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
  let fullscreen = false;
  const dependencies = {
    react: { ...hooks, default: hooks, __esModule: true }, "react/jsx-runtime": jsx, "lucide-react": icons,
    "react-i18next": { useTranslation: () => ({ t }) },
    "./SignalPath": { SignalPath }, "./PlexImage": { PlexImage: () => null },
    "./visualizer/VisualizerPanel": { VisualizerPanel: () => null },
    "../utils/frontendDiagnostics": { logError() { assert.fail("Unexpected diagnostic"); } },
    "../services/audio": { audioService: Object.fromEntries(["togglePlay", "next", "previous", "seek", "setVolume"].map(key => [key, () => { playbackCalls.push(key); }])) },
    "@tauri-apps/api/event": {
      listen: async (name, callback) => { listeners[name] = callback; return () => { delete listeners[name]; }; },
      emitTo: async () => {},
    },
    "@tauri-apps/api/window": { getCurrentWindow: () => ({ isFullscreen: async () => fullscreen, setFullscreen: async value => { fullscreen = value; } }) },
    "../services/nowPlayingWindow": {
      NOW_PLAYING_SNAPSHOT_EVENT: "snapshot", NOW_PLAYING_READY_EVENT: "ready", AUDIO_LEVEL_EVENT: "levels", ANALYZER_STATUS_EVENT: "analyzer",
      startAudioAnalyzer: async () => {}, stopAudioAnalyzer: async () => {},
    },
  };
  const exports = {};
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, exports);
  const originalWindow = globalThis.window;
  globalThis.window = { crypto: { randomUUID: () => "isolated-test" }, clearTimeout() {} };
  let cleanups = [];
  try {
    let tree;
    const render = () => { index = 0; tree = exports.NowPlayingWindow(); return renderToStaticMarkup(tree); };
    const nodes = element => React.isValidElement(element) ? [element, ...nodes(element.props.children)] : Array.isArray(element) ? element.flatMap(nodes) : [];
    render();
    cleanups = effects.splice(0).map(effect => effect());
    for (let i = 0; i < 10; i++) await Promise.resolve();
    const playback = { ...status, title: "Track", artist: "Artist", album: "Album", elapsed: 0, duration: 60, volume: { ...status.volume, available: true, writable: true, value: 50, muted: false } };
    listeners.snapshot({ payload: { health, playback, route: configuredSignalRoute(config) } });
    const direct = render();
    assert.match(direct, /ALSA hw/);
    assert.ok(direct.indexOf('aria-label="Signal Path"') < direct.indexOf("<h2"), "Signal Path precedes the right-hand title");
    const coverColumn = nodes(tree).find(node => node.type === "div" && node.props.className?.includes("max-w-[min(58vh,560px)]"));
    assert.ok(coverColumn);
    const coverChildren = React.Children.toArray(coverColumn.props.children);
    assert.equal(coverChildren[0].type, "div");
    assert.match(coverChildren[0].props.className, /aspect-square w-full shrink-0/);
    assert.equal(coverChildren[1].type, SignalPath, "Panel is directly below the cover inside the same width-limited column");
    assert.equal(nodes(tree).filter(node => node.type === SignalPath).length, 1);
    const grid = nodes(tree).find(node => node.type === "section" && node.props.className.includes("grid"));
    assert.match(grid.props.className, /overflow-x-hidden/);
    assert.match(grid.props.className, /minmax\(0,0\.75fr\).*minmax\(0,1\.25fr\)/);
    listeners.snapshot({ payload: { health, playback: { ...playback, state: "pause", current_media: { kind: "plex" } }, route: configuredSignalRoute({ ...config, audio_output_type: "pipewire" }) } });
    const paused = render();
    assert.match(paused, /ALSA default/); assert.match(paused, /Paused/); assert.ok(paused.includes(t("player.source.plex")));
    listeners.snapshot({ payload: { health: { state: "unavailable", reason: "socket_unavailable" }, playback, route: configuredSignalRoute(config) } });
    const unavailable = render();
    assert.match(unavailable, /MPD unavailable/); assert.doesNotMatch(unavailable, /96 kHz|24-bit/);
    const fullscreenButton = nodes(tree).find(node => node.type === "button" && node.props.title === t("nowPlaying.enterFullscreen"));
    assert.ok(fullscreenButton);
    fullscreenButton.props.onClick();
    for (let i = 0; i < 5; i++) await Promise.resolve();
    const fullscreenHtml = render();
    assert.match(fullscreenHtml, /max-w-\[min\(44vh,500px\)\]/);
    assert.doesNotMatch(fullscreenHtml, /grid-rows-\[minmax\(0,1fr\)\]/);
    assert.deepEqual(playbackCalls, []);
  } finally {
    cleanups.forEach(cleanup => cleanup?.());
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  }
});

test("compact signal uses at most two discreet lines and keeps absent/unknown formats in help", async () => {
  for (const language of ["en", "pt-BR"]) {
    const i18n = i18next.createInstance();
    await i18n.init({ lng: language, resources: { [language]: { translation: resource(language) } }, interpolation: { escapeValue: false } });
    const t = i18n.t.bind(i18n), SignalPath = signalComponent(t);
    const cases = [
      [status, health, t("signalPath.pcmCompact", { rate: 96, bits: 24 })],
      [{ ...status, state: "pause" }, health, `${t("player.source.local")} · ${t("signalPath.paused")}`],
      [{ ...status, state: "stop" }, health, `${t("player.source.local")} · ${t("signalPath.stopped")}`],
      [{ ...status, audio_format: "" }, health, t("player.source.local")],
      [{ ...status, audio_format: "invalid" }, health, t("player.source.local")],
      [status, { state: "unavailable" }, t("signalPath.unavailable")],
      [{ ...status, current_media: null, audio_format: "" }, health, null],
    ];
    for (const [playback, mpdHealth, expected] of cases) {
      const tree = SignalPath({ route: { mode: "direct", endpoint: "hw:Fixture,0" }, status: playback, health: mpdHealth, compact: true });
      const visible = React.Children.toArray(tree.props.children).filter(node => node.props.role !== "tooltip");
      assert.ok(visible.length <= 2);
      assert.equal(visible[0].props.children, t("signalPath.direct"));
      assert.equal(visible[1]?.props.children ?? null, expected);
      for (const node of visible) {
        assert.notEqual(node.props.children, t("signalPath.noSignal"));
        assert.match(node.props.className, /truncate/); assert.ok(node.props.title);
      }
      assert.match(tree.props.className, /group\/compact-signal/);
      assert.ok(tree.props["aria-label"].includes(t("signalPath.title")));
      assert.equal(tree.props["aria-description"], t("signalPath.help"));
    }
  }
});
