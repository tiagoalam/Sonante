import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import i18next from "i18next";
import { routeConditions } from "./routeConditions.ts";
import { configuredSignalRoute, parseReportedMpdFormat, signalPathInformation, requiresDeviceVolumeControl } from "./signalPath.ts";
import { currentMediaKey } from "./playerNavigation.ts";

const config = { first_run: false, audio_output_type: "alsa", alsa_device: "hw:CARD=Fixture,DEV=0", plex_token: "secret-token", local_folders: ["/home/private/music"] };
const status = { state: "play", audio_format: "96000:24:2", current_media: { kind: "local", uri: "private.cue/track0001" }, volume: { backend: "mpd_software" } };
const health = { state: "available" };
const resource = language => JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url)));
function signalComponent(t, view="SignalPath") {
  const source = readFileSync(new URL("../components/SignalPath.tsx", import.meta.url), "utf8");
  const js = ts.transpileModule(source, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  const dependencies = {
    react: { ...React, default: React }, "react/jsx-runtime": jsx, "lucide-react": icons,
    "react-i18next": { useTranslation: () => ({ t }) }, "../utils/signalPath": { signalPathInformation, requiresDeviceVolumeControl }, "../utils/routeConditions": { routeConditions },
  };
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, exports);
  return exports[view];
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

test("known decoded PCM structures are parsed, without codec or source quality inference", () => {
  for (const [value, rate, bits] of [["44100:16:2", 44.1,16], ["96000:24:2",96,24], ["192000:24:2",192,24]]) {
    assert.deepEqual(parseReportedMpdFormat(value), { kind: "pcm", rateKhz: rate, bits, channels: 2 });
  }
  assert.deepEqual(parseReportedMpdFormat(""), { kind: "absent" });
  for (const value of ["48000:dop:2", "48000:f:2", "FLAC", "0:24:2", "96000:24:0", "96000:24:999", "secret /private"]) {
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
      assert.doesNotMatch(html, /data-route-conditions/); assert.match(html, /aria-haspopup="dialog"/);
      assert.match(html, /<ol/);
      assert.match(html, /np-signal-format/); assert.match(html, /width="24"/);
      for (const key of ["rate", "depth", "channels"]) assert.ok(html.includes(t(`signalPath.${key}`)));
      assert.match(html, /<dd[^>]*>2<\/dd>/);
      assert.ok(html.includes(`grid-template-columns:repeat(${mode === "shared" ? 5 : 4}, minmax(0, 1fr))`));
      assert.doesNotMatch(html, /flex-col items-center gap-1 rounded-lg/);
      assert.equal(html.includes(`${t("signalPath.endpointShort")}:`), mode === "direct");
      assert.ok(html.includes(mode === "shared" ? "ALSA default" : "ALSA hw"));
      assert.ok(html.includes(t("signalPath.reported")));
      assert.doesNotMatch(html, /<p[^>]*>Configured route|<p[^>]*>Rota configurada/);
      const noEndpoint = renderToStaticMarkup(React.createElement(SignalPath, { route: { mode: "direct", endpoint: null }, status, health }));
      assert.ok(!noEndpoint.includes(`${t("signalPath.endpointShort")}:`));
      const stopped = renderToStaticMarkup(React.createElement(SignalPath, { route, status: { ...status, state: "stop" }, health }));
      assert.ok(stopped.includes(t("signalPath.stopped"))); assert.doesNotMatch(stopped, /96 kHz|24-bit/);
      assert.doesNotMatch(html, /secret-token|private\.cue|\/home\/private/);
      const compact = renderToStaticMarkup(React.createElement(signalComponent(t,"SignalPath"), { route, status, health, compact: true }));
      assert.match(compact, /aria-haspopup="dialog"/); assert.match(compact, /aria-label=/);
      assert.match(compact, /<button/); assert.match(compact, /96 kHz/); assert.match(compact, /24-bit/); assert.doesNotMatch(compact, /<input|<details/);
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
  assert.match(snapshotEffect, /playback: presentationPlayback,/);
  assert.match(snapshotEffect, /queueArtwork,/);
  assert.match(snapshotEffect, /\[config, mpdHealth, presentationPlayback, sourceInfo, queueArtwork, publishCurrentNowPlayingSnapshot\]/);
  assert.doesNotMatch(snapshotEffect, /settingsDraft|setInterval|setTimeout|invoke/);
  const before = { route: configuredSignalRoute(config), playback: status, health };
  const draft = { ...config, audio_output_type: "pipewire" };
  assert.equal(before.route.mode, "direct");
  const after = { ...before, route: configuredSignalRoute(draft) };
  assert.equal(JSON.parse(JSON.stringify(after)).route.mode, "shared");
  const window = readFileSync(new URL("../components/NowPlayingWindow.tsx", import.meta.url), "utf8");
  assert.match(window, /register<NowPlayingSnapshot>\(NOW_PLAYING_SNAPSHOT_EVENT/);
  const presentation = readFileSync(new URL("../components/NowPlayingPresentation.tsx", import.meta.url), "utf8");
  assert.match(presentation, /<SignalPath route=\{props.snapshot\?\.route \?\? null\}/);
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
    "../utils/signalPath": { requiresDeviceVolumeControl },
    "./SignalPath": { SignalPath }, "./TrackArtwork": { TrackArtwork: () => null },
    "./SourceAudioBadges": { SourceAudioBadges: () => null }, "../utils/playerNavigation": { currentMediaKey },
    "./NowPlayingQueue": { NowPlayingQueue: () => null }, "./nowPlaying.css": {},
    "../utils/nowPlayingHosting": { analyzerOwnerMatches: () => true },
    "./NowPlayingHost": { NowPlayingHost: ({ snapshot }) => snapshot ? React.createElement(SignalPath, { route: snapshot.route, status: snapshot.playback, health: snapshot.health, horizontal: true }) : null },
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
  const presentationSource = readFileSync(new URL("../components/NowPlayingPresentation.tsx", import.meta.url), "utf8");
  const presentationJs = ts.transpileModule(presentationSource, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
  const presentationExports = {};
  new Function("require", "exports", presentationJs)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, presentationExports);
  dependencies["./NowPlayingPresentation"] = presentationExports;
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
    listeners.snapshot({ payload: { health, playback: { ...playback, state: "pause", current_media: { kind: "plex" } }, route: configuredSignalRoute({ ...config, audio_output_type: "pipewire" }) } });
    const paused = render();
    assert.match(paused, /ALSA default/); assert.match(paused, /Paused/); assert.ok(paused.includes(t("player.source.plex")));
    listeners.snapshot({ payload: { health: { state: "unavailable", reason: "socket_unavailable" }, playback, route: configuredSignalRoute(config) } });
    const unavailable = render();
    assert.match(unavailable, /MPD unavailable/); assert.doesNotMatch(unavailable, /96 kHz|24-bit/);
    assert.deepEqual(playbackCalls, []);
  } finally {
    cleanups.forEach(cleanup => cleanup?.());
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  }
});

test("compact route restores mode and audio format while keeping diagnostics in the dialog",async()=>{
 for(const language of ["en","pt-BR"]){
  const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:resource(language)}}});
  const t=i18n.t.bind(i18n),Main=signalComponent(t,"SignalPath");
  for(const state of ["play","pause","stop"]){
   const html=renderToStaticMarkup(React.createElement(Main,{route:{mode:'direct',endpoint:'hw:Fixture,0'},status:{...status,state},health,compact:true}));
   assert.match(html,/data-signal-summary/);assert.match(html,/aria-haspopup="dialog"/);
   assert.doesNotMatch(html,/hw:Fixture|<dialog|data-route-conditions/);
   if(state==="play") { assert.match(html,/96 kHz/); assert.match(html,/24-bit/); }
   if(state==="stop") assert.doesNotMatch(html,/kHz|24-bit/);
   if(state!=="play")assert.ok(html.includes(t(`signalPath.${state==='pause'?'paused':'stopped'}`)));
  }
 }
});

test("reported DSD short and byte-rate forms are normalized independently of routes or source codec", () => {
 for (const level of [32,64,128,256,512,1024]) {
  const expected={kind:"dsd",rateMhz:44100*level/1e6,level,channels:2};
  assert.deepEqual(parseReportedMpdFormat(`dsd${level}:2`),expected);
  assert.deepEqual(parseReportedMpdFormat(`${44100*level/8}:dsd:2`),expected);
 }
 assert.deepEqual(parseReportedMpdFormat("384000:dsd:6"),{kind:"dsd",rateMhz:3.072,level:null,channels:6});
 for(const value of ["dsd:2","dsd63:2","dsd64:0","dsd64:65","dsd64:2:1","DSD64:2","0:dsd:2","352800:dsd:0","352800:dsd:999","99999999999999999:dsd:2","352800.5:dsd:2"," ds d64:2","352800:DoP:2"])
  assert.deepEqual(parseReportedMpdFormat(value),{kind:"unknown"},value);
});

test("DSD report appears in compact and detailed SignalPath without PCM depth, Hi-Res or DAC assertions",async()=>{
 for(const language of ["en","pt-BR"]){
  const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:resource(language)}},interpolation:{escapeValue:false}});
  const t=i18n.t.bind(i18n),SignalPath=signalComponent(t);
  for(const mode of ["shared","direct"])for(const kind of ["local","plex"])for(const level of [64,128,256]) {
   const route={mode,endpoint:mode==="shared"?"default":"hw:Fixture,0"};
   const playback={...status,audio_format:`dsd${level}:2`,current_media:{kind}};
   for(const props of [{},{horizontal:true},{compact:true}]){
    const html=renderToStaticMarkup(React.createElement(SignalPath,{route,status:playback,health,...props}));
    assert.ok(html.includes(`DSD${level}`));assert.ok(html.includes(`${44100*level/1e6} MHz`));

    assert.ok(!html.includes(t("signalPath.unknownFormat")));assert.doesNotMatch(html,/Hi-Res|1-bit|16-bit|24-bit|48 kHz|data-source-audio/);
    if(!props.compact){assert.ok(html.includes(t("signalPath.reported")));assert.ok(html.includes(t("signalPath.format")));assert.ok(!html.includes(t("signalPath.depth")));assert.match(html,/<dd[^>]*>2<\/dd>/);}
   }
   for(const change of [{status:{...playback,state:"stop"}},{health:{state:"unavailable"}},{status:{...playback,audio_format:""}}]){
    const html=renderToStaticMarkup(React.createElement(SignalPath,{route,status:playback,health,...change}));assert.doesNotMatch(html,/DSD64|DSD128|DSD256|MHz/);
   }
  }
 }
});

test("observed ALSA containers stay separate from MPD input and never certify bit-perfect, DoP or native DSD", async () => {
  const observation = { state: "observed", configured_endpoint: config.alsa_device, card: 2, device: 0, subdevice: 0, format: "S32_LE", rate: 192000, channels: 2 };
  for (const language of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng: language, resources: { [language]: { translation: resource(language) } }, fallbackLng: false });
    const t = instance.t.bind(instance), SignalPath = signalComponent(t,"SignalPathDetails");
    for (const kind of ["local", "plex"]) for (const horizontal of [false, true]) {
      const playback = { ...status, current_media: { kind }, output_observation: observation };
      const html = renderToStaticMarkup(React.createElement(SignalPath, { route: configuredSignalRoute(config), status: playback, health, horizontal }));
      assert.match(html, /data-alsa-observation="observed"/);
      assert.ok(html.includes(t("signalPath.observation.observedTitle")));
      assert.match(html, /hw:2,0 · S32_LE · 192 kHz/);
      assert.doesNotMatch(html, /np-signal-format|np-signal-stages/);
      assert.ok(html.includes(t("signalPath.observation.notVerified")));
      assert.doesNotMatch(html, /data-bitperfect|owner_pid|verified-badge/);
      const compact = renderToStaticMarkup(React.createElement(signalComponent(t,"SignalPath"),{route:configuredSignalRoute(config),status:playback,health,compact:true}));
      assert.doesNotMatch(compact,/S32_LE|192 kHz|hw:2,0/);assert.match(compact,/aria-haspopup="dialog"/);
      for (const change of [{ status: { ...playback, state: "pause" } }, { status: { ...playback, state: "stop" } }, { health: { state: "unavailable" } }, { route: { mode: "shared", endpoint: "default" } }, { route: { mode: "direct", endpoint: "hw:Other,0" } }]) {
        const stale = renderToStaticMarkup(React.createElement(SignalPath, { route: configuredSignalRoute(config), status: playback, health, ...change }));
        assert.match(stale, /data-alsa-observation="unverified"/);
        assert.doesNotMatch(stale, /S32_LE|192 kHz|hw:2,0/);
      }
    }
    for (const reason of ["unavailable", "not_playing", "shared", "unreadable", "malformed", "not_owned", "ambiguous", "changed", "limit", "busy"]) {
      const html = renderToStaticMarkup(React.createElement(SignalPath, { route: configuredSignalRoute(config), status: { ...status, output_observation: { state: "unverified", reason } }, health }));
      assert.ok(html.includes(t(`signalPath.observation.${reason}`)));
      assert.doesNotMatch(html, /signalPath\./);
    }
  }
});

test("actual App status polling carries and clears ALSA and engine evidence through its existing request and timer", async () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let effect;
  const visit = node => {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect" && node.arguments[0]?.getText(ast).includes("audioService.getMpdStatusSnapshot()")) effect = node.arguments[0].getText(ast);
    ts.forEachChild(node, visit);
  }; visit(ast); assert.ok(effect);
  const observation = { state: "observed", configured_endpoint: config.alsa_device, card: 2, device: 0, subdevice: 0, format: "S32_LE", rate: 96000, channels: 2 };
  const identified = { state: "identified", evidence_id: "mpd-pcm-eof-20261017", scope: "pcm24_96000_stereo_gapless", pcm_eof_flush: true };
  for (const output of [observation, null, undefined]) for (const engine of [identified, null, undefined]) {
    let requests = 0, timers = 0; const values = [], updates = [];
    const vars = { audioService: { async getMpdStatusSnapshot() { requests++; return { health, playback: status, output_observation: output, engine_evidence: engine }; } }, statusRequestGenerationRef: { current: 0 },
      localAlbumCatalog: { onMpdStatus: value => updates.push(value) }, setMpdHealth() {}, setPlaybackStatus: value => values.push(value), logError() { assert.fail("unexpected error"); },
      window: { setTimeout(fn, milliseconds) { assert.equal(milliseconds,1000); timers++; return 1; }, clearTimeout() {} } };
    const effectJs = ts.transpileModule(`const effect = (${effect});`, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
    const cleanup = new Function(...Object.keys(vars), `${effectJs}; return effect();`)(...Object.values(vars));
    for (let i=0;i<10;i++) await Promise.resolve();
    assert.equal(requests,1); assert.equal(timers,1); assert.equal(values.length,1);
    assert.deepEqual(values[0], { ...status, output_observation: output ?? null, engine_evidence: engine ?? null, shared_dsp: null });
    assert.equal(values[0].output_observation, output ?? null);
    assert.deepEqual(status.output_observation, undefined); assert.equal(status.engine_evidence, undefined); cleanup();
  }
});


test("the grid reserves one column per real stage, including Shared EQ's sixth output stage", async () => {
  for (const language of ["en", "pt-BR"]) {
    const i18n = i18next.createInstance();
    await i18n.init({ lng: language, resources: { [language]: { translation: resource(language) } } });
    const SignalPath = signalComponent(i18n.t.bind(i18n));
    for (const horizontal of [false, true]) {
      const playback = { ...status, shared_dsp: { in_path: true, phase: "active", verified: true } };
      const html = renderToStaticMarkup(React.createElement(SignalPath, { route: { mode: "shared", endpoint: "default" }, status: playback, health, horizontal }));
      const stages = html.match(/<ol[^>]*>(.*?)<\/ol>/s)[1];
      assert.equal((stages.match(/<li /g) ?? []).length, 6);
      assert.match(html, /grid-template-columns:repeat\(6, minmax\(0, 1fr\)\)/);
      assert.ok(stages.includes(i18n.t("signalPath.output")));
      assert.ok(stages.includes(i18n.t("equalizer.state.active")));
      assert.match(stages, /PipeWire/);
    }
  }
});

test('Pending describes normal playback authorization without granting a seal or hiding unrelated differences',async()=>{
 const route={mode:'direct',endpoint:'hw:Fixture,0'};const playback={...status,audio_format:'44100:16:2',snapshot_consistent:true,volume:{backend:'direct_equipment',available:true,writable:false,value:0,muted:false,direct_gain:'pending',direct_token:'1:1'},engine_evidence:{state:'identified',evidence_id:'mpd-strict-ubuntu2204-remap-20261018',scope:'pcm_dsd_route_strict',pcm_eof_flush:true},output_observation:{state:'observed',configured_endpoint:route.endpoint,card:0,device:0,subdevice:0,format:'S32_LE',rate:44100,channels:2},processing:{mpd_volume:0,crossfade_seconds:0,mixramp:'off',replay_gain:{state:'off'}}};
 const evidence=routeConditions(route,playback,health);assert.equal(evidence.bitPerfect,false);assert.equal(evidence.assessment,'partial');assert.deepEqual(evidence.issues.map(r=>r.message),['directPending']);
 for(const lng of['en','pt-BR']){const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:resource(lng)}}});const t=i18n.t.bind(i18n);
  for(const view of['SignalPath','SignalPathDetails']){const C=signalComponent(t,view),html=renderToStaticMarkup(React.createElement(C,{route,status:playback,health,compact:true}));assert.ok(html.includes(t('signalPath.assessment.directPending')));assert.doesNotMatch(html,/data-bitperfect="route"/);assert.ok(!html.includes(t('signalPath.conditions.summary.attention')));
   const other=renderToStaticMarkup(React.createElement(C,{route,status:{...playback,processing:{...playback.processing,crossfade_seconds:2}},health,compact:true}));assert.ok(other.includes(t('signalPath.assessment.attention')));
  }
 }
});
