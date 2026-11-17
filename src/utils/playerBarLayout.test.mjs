import * as directVolumeHelpers from "./signalPath.ts";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import ts from "typescript";
import i18next from "i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { signalPathInformation, requiresDeviceVolumeControl } from "./signalPath.ts";
import { currentMediaKey } from "./playerNavigation.ts";
import { sourceAudioBadge, albumAudioBadge } from "./sourceAudio.ts";

const resource = language => JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url)));
const source = name => readFileSync(new URL(`../components/${name}.tsx`, import.meta.url), "utf8");
const status = { state: "play", title: "A very long track title ".repeat(12), artist: "An artist ".repeat(10), album: "An album ".repeat(10), audio_format: "192000:24:2", current_media: { kind: "local" }, options: { repeat: false, single: "off", random: false }, elapsed: 12, duration: 180, volume: { backend: "mpd_software", available: true, writable: true, value: 50, muted: false, shared_token: "layout-context-A" } };
const iconSizeOf = button => React.Children.toArray(button.props.children)[0].props.size;
const nodes = element => React.isValidElement(element) ? [element, ...nodes(element.props.children)] : Array.isArray(element) ? element.flatMap(nodes) : [];
function compile(name, dependencies) {
  const exports = {};
  const js = ts.transpileModule(source(name), { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, exports);
  return exports[name];
}
async function fixture(language = "en", sharedMuteError = null) {
  const i18n = i18next.createInstance();
  await i18n.init({ lng: language, resources: { [language]: { translation: resource(language) } }, interpolation: { escapeValue: false } });
  const t = i18n.t.bind(i18n), calls = [], diagnostics = [];
  const dependencies = { react: { ...React, default: React, __esModule: true }, "react/jsx-runtime": jsx, "lucide-react": icons, "react-i18next": { useTranslation: () => ({ t }) }, "../utils/signalPath": {...directVolumeHelpers, signalPathInformation, requiresDeviceVolumeControl } };
  const SignalPath = compile("SignalPath", dependencies);
  const SourceAudioBadges = compile("SourceAudioBadges", { ...dependencies, "../utils/sourceAudio": { sourceAudioBadge, albumAudioBadge } });
  const hooks = { ...React, useState: initial => [initial, () => {}], useRef: initial => ({ current: initial }), useEffect() {} };
  const SleepTimerControl = () => React.createElement("button", { type: "button", "data-player-action": "timer", className: "h-9 w-9", "aria-label": "Timer" });
  const controlled = { ...dependencies,
    "./SleepTimerControl": { SleepTimerControl },
    react: { ...hooks, default: hooks, __esModule: true },
    "./SignalPath": { SignalPath }, "./PlexImage": { PlexImage: () => null },
    "./SourceAudioBadges": { SourceAudioBadges }, "../utils/playerNavigation": { currentMediaKey },
    "../utils/frontendDiagnostics": { logError(...args) { if (!sharedMuteError) assert.fail("Unexpected diagnostic"); diagnostics.push(args); } },
    "../services/audio": { audioService: Object.fromEntries(["togglePlay", "next", "previous", "seek", "setVolume", "setDirectMute", "setSharedMute", "changePlaybackOption"].map(name => [name, async (...args) => { calls.push([name, ...args]); if (name === "setSharedMute" && sharedMuteError) throw sharedMuteError; }])) },
  };
  const PlaybackOptionsControls = compile("PlaybackOptionsControls", controlled);
  const PlayerBar = compile("PlayerBar", { ...controlled, "./PlaybackOptionsControls": { PlaybackOptionsControls }, "./playerBar.css": {} });
  const props = { status, health: { state: "available" }, route: { mode: "direct", endpoint: "hw:Fixture,0" }, isQueueOpen: false,
    onNavigateToAlbum: () => calls.push(["album"]), onNavigateToArtist: () => calls.push(["artist"]), onToggleQueue: () => calls.push(["queue"]), onOpenNowPlaying: () => calls.push(["now-playing"]) };
  return { t, calls, diagnostics, props, SignalPath, SleepTimerControl, PlaybackOptionsControls, render: changes => PlayerBar({ ...props, ...changes }) };
}

const playerCss = readFileSync(new URL("../components/playerBar.css", import.meta.url), "utf8");
function geometry(tree, width) {
  assert.match(tree.props.className, /player-compact-grid/);
  assert.match(playerCss, /grid-template-areas:\s*"metadata transport state-volume actions"/);
  const tracks = /grid-template-columns:\s*minmax\(0,\s*1fr\)\s+clamp\((\d+)px,\s*(\d+)vw,\s*(\d+)px\)\s+minmax\(0,\s*1fr\)\s+(\d+)px/.exec(playerCss);
  assert.ok(tracks, "Desktop grid defines flexible A/C rails, transport B and fixed action D");
  const padding = 16, gap = 12;
  const transportWidth = Math.max(Number(tracks[1]), Math.min(width * Number(tracks[2]) / 100, Number(tracks[3])));
  const actionWidth = Number(tracks[4]);
  const sideWidth = (width - 2 * padding - 3 * gap - transportWidth - actionWidth) / 2;
  const metadataLeft = padding, transportLeft = metadataLeft + sideWidth + gap;
  const stateLeft = transportLeft + transportWidth + gap, actionsLeft = stateLeft + sideWidth + gap;
  return { sideWidth, transportWidth, actionWidth, metadataLeft, transportLeft, stateLeft, actionsLeft,
    transportCenter: transportLeft + transportWidth / 2, stateCenter: stateLeft + sideWidth / 2,
    gap, padding, width };
}

test("four explicit player regions center state/volume between transport and actions at desktop widths", async () => {
  const { render, SignalPath, SleepTimerControl, PlaybackOptionsControls } = await fixture();
  for (const width of [800, 900, 1024, 1366, 1920]) for (const mode of ["shared", "direct"]) for (const state of ["play", "pause"]) {
    const tree = render({ status: { ...status, state }, route: { mode, endpoint: mode === "shared" ? "default" : "hw:Fixture,0" } });
    const regions = React.Children.toArray(tree.props.children);
    assert.deepEqual(regions.map(region => region.props["data-player-area"]), ["metadata", "transport", "state-volume", "actions"]);
    const [metadata, transport, stateVolume, actions] = regions;
    const layout = geometry(tree, width);
    assert.ok(layout.sideWidth > 0, `${width}px keeps all four regions in the viewport`);
    assert.ok(Math.abs(layout.stateCenter - ((layout.transportLeft + layout.transportWidth + layout.actionsLeft) / 2)) < 1e-9,
      "Region C center equals the midpoint from B's right edge to D's left edge");
    const controls = nodes(transport).find(node => node.props["data-player-controls"] !== undefined);
    const transportButtons = React.Children.toArray(controls.props.children).filter(node => node.type === "button");
    assert.equal(transportButtons.length, 3);
    const optionNode = nodes(controls).find(node => node.type === PlaybackOptionsControls);
    const options = PlaybackOptionsControls(optionNode.props);
    const optionButtons = nodes(options).filter(node => node.type === "button");
    assert.equal(optionButtons.length, 2);
    const playWidth = 40;
    const controlsTrack = /grid-cols-\[minmax\(0,1fr\)_(\d+)px_(\d+)px_(\d+)px_minmax\(0,1fr\)\]/.exec(controls.props.className);
    assert.ok(controlsTrack);
    const fixed = controlsTrack.slice(1).map(Number), gap = 6;
    const side = (layout.transportWidth - fixed.reduce((a, b) => a + b, 0) - 4 * gap) / 2;
    const playCenter = layout.transportLeft + side + gap + fixed[0] + gap + fixed[1] / 2;
    assert.ok(Math.abs(playCenter - layout.transportCenter) < 1e-9, "Play is centered in B");
    assert.ok(nodes(metadata).some(node => node.props["data-player-cover"] !== undefined));
    const signal = nodes(stateVolume).find(node => node.type === SignalPath);
    assert.equal(signal.props.compact, true);
    const signalTree = SignalPath(signal.props);
    const chips = ["data-signal-route", "data-signal-state"].map(key => nodes(signalTree).find(node => node.props[key] !== undefined));
    assert.ok(chips.every(Boolean));
    assert.ok(chips.every(node => node.props.className.includes("h-6")));
    const volumeControls = nodes(stateVolume).find(node => node.props["data-player-volume-controls"] !== undefined);
    assert.ok(volumeControls);
    const actionsList = ["expand", "queue"].map(action => nodes(actions).find(node => node.props["data-player-action"] === action));
    const timerNode = nodes(actions).find(node => node.type === SleepTimerControl);
    assert.ok(timerNode);
    actionsList.push(SleepTimerControl(timerNode.props));
    assert.ok(actionsList.every(Boolean));
    for (const action of actionsList) assert.match(action.props.className, /h-9 w-9/);
    assert.ok(layout.actionWidth === 120);
    assert.doesNotMatch(playerCss, /position:\s*absolute|transform:|margin-left:\s*-/);
    assert.doesNotMatch(renderToStaticMarkup(tree), /overflow-x-auto|overflow-x-scroll/);
    if (mode === "shared") {
      const seek = nodes(transport).find(node => node.props["data-player-seek"] !== undefined);
      const volume = nodes(stateVolume).find(node => node.props["data-player-volume-slider"] !== undefined);
      const percent = nodes(stateVolume).find(node => node.props["data-player-volume-percent"] !== undefined);
      assert.ok(seek && volume && percent);
      assert.equal(React.Children.toArray(percent.props.children).join(""), "50%");
      assert.equal(volume.props.value, 50);
      assert.equal(volume.props["aria-label"], "Volume");
      assert.match(volumeControls.props.className, /has-slider/);
    } else {
      assert.ok(nodes(stateVolume).some(node => node.props["data-player-volume-status"] !== undefined));
      assert.equal(nodes(stateVolume).some(node => node.props["data-player-volume-slider"] !== undefined), false);
      assert.equal(nodes(stateVolume).some(node => node.props["data-player-volume-percent"] !== undefined), false);
    }
  }
});

test("volume uses a stable percentage slot and grows to the width available in Region C", async () => {
  const { render } = await fixture();
  for (const width of [500, 800, 900, 1024, 1366, 1920]) {
    const layout = geometry(render({ route: { mode: "shared", endpoint: "default" } }), width);
    const musicTree = render({ route: { mode: "shared", endpoint: "default" } });
    const volume = nodes(musicTree).find(node => node.props["data-player-volume-slider"] !== undefined);
    const percent = nodes(musicTree).find(node => node.props["data-player-volume-percent"] !== undefined);
    assert.ok(volume && percent, "Shared mode exposes an available slider and its reported percentage");
    assert.equal(percent.props.className, "player-compact-volume-percent");
    assert.match(playerCss, /grid-template-columns:\s*36px minmax\(0,\s*1fr\) 4ch/);
    assert.match(playerCss, /width:\s*clamp\(100px,\s*16vw,\s*160px\)/);
    const regionWidth = width <= 560 ? 280 : Math.min(280, layout.sideWidth);
    const slotWidth = Math.max(0, regionWidth - 36 - 32 - 12);
    const desiredWidth = Math.max(100, Math.min(160, width * 0.16));
    const availableWidth = Math.min(desiredWidth, slotWidth);
    assert.ok(availableWidth > 0, `${width}px keeps the slider inside the Region C volume row`);
    if (width >= 900) assert.ok(availableWidth >= 140, `${width}px provides a legible desktop slider`);
    assert.ok(!renderToStaticMarkup(musicTree).includes("overflow-x-scroll"));
  }
});

test("volume percentage follows only a reported value and is omitted when volume is unavailable", async () => {
  const { render } = await fixture();
  const shared = render({ route: { mode: "shared", endpoint: "default" }, status: { ...status, volume: { ...status.volume, value: 65 } } });
  assert.equal(React.Children.toArray(nodes(shared).find(node => node.props["data-player-volume-percent"] !== undefined).props.children).join(""), "65%");
  const unavailable = render({ route: { mode: "shared", endpoint: "default" }, status: { ...status, volume: { ...status.volume, available: false, backend: "unavailable" } } });
  assert.equal(nodes(unavailable).some(node => node.props["data-player-volume-slider"] !== undefined), false);
  assert.equal(nodes(unavailable).some(node => node.props["data-player-volume-percent"] !== undefined), false);
});

test("narrow player reflows transport across both columns before reducing the progress track", () => {
  assert.match(playerCss, /@media \(max-width:\s*560px\)[\s\S]*?grid-template-areas:\s*"metadata actions" "transport transport" "state-volume state-volume"/);
  assert.match(playerCss, /@media \(max-width:\s*560px\)[\s\S]*?grid-template-columns:\s*minmax\(0,\s*1fr\) 120px/);
  assert.match(playerCss, /@media \(max-width:\s*560px\)[\s\S]*?--player-compact-height:\s*176px/);
});

test("compact player header aligns the configured route, live MPD state and volume information", async () => {
  for (const language of ["en", "pt-BR"]) {
    const { render, SignalPath, t } = await fixture(language);
    const tree = render({ status: { ...status, state: "pause" } });
    const signalNode = nodes(tree).find(node => node.type === SignalPath);
    const compactSignal = SignalPath(signalNode.props);
    const route = nodes(compactSignal).find(node => node.props["data-signal-route"]);
    const playback = nodes(compactSignal).find(node => node.props["data-signal-state"]);
    const volume = nodes(tree).find(node => node.props["data-player-volume-status"]);
    assert.ok(route && playback && volume);
    for (const item of [route, playback, volume]) assert.match(item.props.className, /h-6/);
    assert.equal(playback.props.children, t("signalPath.pausedCompact"));
    assert.equal(volume.props.children, t("player.volumeOnDeviceShort"));
    assert.ok(volume.props.title.includes(t("player.volumeOnDevice")));
    const html = renderToStaticMarkup(tree);
    assert.ok(html.includes(t("signalPath.direct")));
    assert.ok(html.includes(t("signalPath.pausedCompact")));
    assert.ok(html.includes(t("player.volumeOnDeviceShort")));

    const unavailable = renderToStaticMarkup(render({ health: { state: "unavailable", reason: "socket_unavailable" } }));
    assert.ok(unavailable.includes(t("signalPath.unavailable")));
    assert.ok(!unavailable.includes(`${t("signalPath.direct")} · ${t("signalPath.playing")}`));
  }
});

test("queue drawer and Save feedback clear the new 112px player without shifting unrelated layouts", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const queue = source("QueueDrawer");
  assert.match(app, /bottom-\[var\(--player-compact-height,112px\)\]/);
  assert.match(queue, /className="np-queue-drawer"/);
  const css = readFileSync(new URL("../components/nowPlaying.css", import.meta.url), "utf8");
  const drawer = css.match(/\.np-queue-drawer\s*\{([^}]+)\}/)[1];
  for (const rule of [/position:\s*fixed/, /top:\s*0/, /right:\s*0/, /bottom:\s*var\(--player-compact-height,112px\)/]) assert.match(drawer, rule);
  assert.doesNotMatch(app, /bottom-24/);
  assert.doesNotMatch(queue, /bottom-24/);
});

test("player routing area restores audio format and keeps detailed route conditions in the dialog",async()=>{
 for(const language of ["en","pt-BR"]){
  const {render,t}=await fixture(language);
  for(const mode of ["shared","direct"])for(const kind of ["local","plex"]){
   const html=renderToStaticMarkup(render({route:{mode,endpoint:mode==='shared'?'default':'hw:Fixture,0'},status:{...status,current_media:{kind}}}));
   assert.ok(html.includes(t("signalPath.title")));assert.match(html,/data-signal-summary|aria-haspopup="dialog"/);
   assert.match(html,/192 kHz/); assert.match(html,/24-bit/);
   assert.doesNotMatch(html,/hw:Fixture|data-route-conditions|<dialog/);
  }
 }
});

test("navigation, playback, seek, volume and actions retain their handlers", async () => {
  const { render, calls, t } = await fixture();
  const tree = render({route:{mode:"shared",endpoint:"default"}}), all = nodes(tree);
  const click = async title => { const button = all.find(node => node.type === "button" && node.props.title === title); assert.ok(button); await button.props.onClick(); };
  await click(status.title); await click(status.album); await click(status.artist);
  await click(t("player.previous")); await click(t("player.pause")); await click(t("player.next"));
  const ranges = all.filter(node => node.type === "input" && node.props.type === "range");
  assert.equal(ranges.length, 2);
  await ranges[0].props.onMouseUp({ target: { value: "42.5" } });
  await ranges[1].props.onChange({ target: { value: "25" } });
  await click(t("player.mute")); await click(t("player.openNowPlaying")); await click(t("player.queue"));
  assert.deepEqual(calls, [["album"], ["album"], ["artist"], ["previous"], ["togglePlay"], ["next"], ["seek", 42.5], ["setVolume", 25], ["setSharedMute", { muted: true, token: status.volume.shared_token }], ["now-playing"], ["queue"]]);
});

test("real repeat/random controls preserve accessible MPD state, submit actions and block unavailable playback", async () => {
  for (const language of ["en", "pt-BR"]) {
    const { render, PlaybackOptionsControls, calls, t } = await fixture(language);
    const optionNode = tree => nodes(tree).find(node => node.type === PlaybackOptionsControls);
    const node = optionNode(render()), tree = PlaybackOptionsControls(node.props);
    const buttons = nodes(tree).filter(node => node.type === "button");
    assert.equal(buttons.length, 2);
    for (const [index, key] of [[0, "player.repeat.off"], [1, "player.random.off"]]) {
      assert.equal(buttons[index].props["aria-label"], t(key));
      assert.equal(buttons[index].props.title, t(key)); assert.notEqual(t(key), key);
      assert.equal(buttons[index].props["aria-pressed"], false);
      buttons[index].props.onClick(); await Promise.resolve(); await Promise.resolve();
    }
    assert.deepEqual(calls, [["changePlaybackOption", "cycle_repeat", status.options], ["changePlaybackOption", "toggle_random", status.options]]);
    assert.deepEqual(status.options, { repeat: false, single: "off", random: false });
    calls.length = 0;
    const unavailable = optionNode(render({ health: { state: "unavailable", reason: "socket_unavailable" } }));
    for (const button of nodes(PlaybackOptionsControls(unavailable.props)).filter(node => node.type === "button")) {
      assert.equal(button.props.disabled, true); button.props.onClick();
    }
    await Promise.resolve(); assert.deepEqual(calls, []);
  }
});

test("paused, stopped and unavailable signal never claims stale data is active", async () => {
  const { render, t, calls } = await fixture();
  for (const state of ["pause", "stop"]) {
    const html = renderToStaticMarkup(render({ status: { ...status, state } }));
    assert.ok(html.includes(t(`signalPath.${state === "pause" ? "paused" : "stopped"}`)));
    if (state === "stop") assert.doesNotMatch(html, /192 kHz|24-bit/);
  }
  const tree = render({ health: { state: "unavailable", reason: "socket_unavailable" } });
  const html = renderToStaticMarkup(tree);
  assert.doesNotMatch(html, /192 kHz|24-bit/); assert.ok(html.includes(t("signalPath.unavailable")));
  for (const node of nodes(tree).filter(node => node.type === "input")) assert.equal(node.props.disabled, true);
  for (const title of [t("player.play"), t("player.previous"), t("player.next")]) {
    const node = nodes(tree).find(node => node.type === "button" && node.props.title === title);
    assert.equal(node.props.disabled, true); await node.props.onClick();
  }
  assert.deepEqual(calls, []);
});

test("simultaneous errors and unavailable status stay concise without dropping their messages", async () => {
  const { render, t } = await fixture();
  const tree = render({ health: { state: "unavailable", reason: "socket_unavailable" }, status: { ...status, playback_error: { code: "output_busy" } } });
  const playback = nodes(tree).find(node => node.props["data-player-area"] === "transport");
  assert.match(playback.props.className, /gap-1$/);
  for (const title of [t("player.playbackError.output_busy"), t("player.engineUnavailable")]) {
    const message = nodes(playback).find(node => node.props.title === title);
    assert.ok(message); assert.match(message.props.className, /truncate.*leading-tight/);
    assert.equal(message.props.children, title);
  }
  const error = nodes(playback).find(node => node.props.role === "status");
  assert.equal(error.props["aria-live"], "polite");
});

test("unmute, unknown format and missing route remain conservative", async () => {
  const { render, calls, t } = await fixture();
  const tree = render({ route: null, status: { ...status, audio_format: "unrecognized private input", volume: { ...status.volume, value: 0, muted: true } } });
  const html = renderToStaticMarkup(tree);
  assert.doesNotMatch(html,/data-route-assessment|data-bitperfect|data-signal-info/);
  assert.doesNotMatch(html, /unrecognized private input/);
  const unmute = nodes(tree).find(node => node.type === "button" && node.props.title === t("player.unmute"));
  assert.ok(unmute); await unmute.props.onClick();
  assert.deepEqual(calls, [["setSharedMute", { muted: false, token: status.volume.shared_token }]]);

  // No context means no request. A valid zero-volume context does submit an
  // unmute request; only the backend can prove a previous level in that context.
  calls.length = 0;
  const noContext = render({ route: null, status: { ...status, volume: { ...status.volume, value: 0, muted: true, shared_token: undefined } } });
  await nodes(noContext).find(node => node.type === "button" && node.props.title === t("player.unmute")).props.onClick();
  assert.deepEqual(calls, []);
  for (const backend of ["mpd_software", "pipewire"]) {
    for (const [value, muted] of [[37, false], [0, true], [37, true]]) {
      calls.length = 0;
      const observed = { ...status, volume: { ...status.volume, backend, value, muted, shared_token: `${backend}-current-context` } };
      const title = t(muted || value === 0 ? "player.unmute" : "player.mute");
      await nodes(render({ route: { mode: "shared", endpoint: "default" }, status: observed })).find(node => node.type === "button" && node.props.title === title).props.onClick();
      assert.deepEqual(calls, [["setSharedMute", { muted: !(muted || value === 0), token: observed.volume.shared_token }]]);
    }
  }
  const unavailable = Error("SHARED_MUTE_LEVEL_UNKNOWN");
  const refused = await fixture("en", unavailable), alerts = [];
  const previousWindow = globalThis.window;
  globalThis.window = { alert: message => alerts.push(message) };
  try {
    const observed = { ...status, volume: { ...status.volume, value: 0, muted: true, shared_token: "zero-without-session-history" } };
    const before = JSON.stringify(observed);
    const button = nodes(refused.render({ route: { mode: "shared", endpoint: "default" }, status: observed })).find(node => node.type === "button" && node.props.title === refused.t("player.unmute"));
    await button.props.onClick();
    assert.deepEqual(refused.calls, [["setSharedMute", { muted: false, token: "zero-without-session-history" }]]);
    assert.deepEqual(refused.diagnostics, [["player.volume", unavailable]]);
    assert.deepEqual(alerts, [refused.t("player.sharedMuteFailed")]);
    assert.equal(JSON.stringify(observed), before);
    assert.ok(!refused.calls.some(([name]) => name === "setVolume" || name === "setDirectMute"));
  } finally {
    if (previousWindow === undefined) delete globalThis.window; else globalThis.window = previousWindow;
  }
});

test("All Direct transports use equipment volume and dedicated mute; Shared stays adjustable",async()=>{
 for(const lng of ['en','pt-BR']){
  const {render,t,calls}=await fixture(lng);
  for(const [mode,audio_format,backend,slider] of [
   ['direct','96000:24:2','direct_equipment',false],['direct','dsd64:2','direct_equipment',false],
   ['direct','352800:dsd:2','direct_equipment',false],['direct','dsd128:2','direct_equipment',false],
   ['shared','dsd64:2','pipewire',true],['shared','96000:24:2','pipewire',true]]){
   const tree=render({route:{mode,endpoint:mode==='direct'?'hw:Fixture,0':'default'},status:{...status,audio_format,volume:{...status.volume,backend,...(mode==='direct'?{value:100,writable:false,direct_gain:'unity',direct_token:'1:65'}:{})}}});
   const ranges=nodes(tree).filter(n=>n.type==='input'&&n.props.type==='range');
   assert.equal(ranges.length,slider?2:1,JSON.stringify({mode,audio_format,backend}));
   const mute=nodes(tree).find(n=>n.type==='button'&&n.props.title===t('player.mute'));assert.ok(mute);assert.equal(mute.props.disabled,false);
   if(!slider){await mute.props.onClick();assert.deepEqual(calls.pop(),['setDirectMute',{muted:true,token:'1:65'}]);const html=renderToStaticMarkup(tree);assert.ok(html.includes(t('player.volumeOnDevice')));}
  }
 }
});

test('current generic MPD errors appear while stopped or paused, then clear after recovery',async()=>{
 const {render,t}=await fixture();
 for(const state of ['pause','stop']) {
  const tree=render({status:{...status,state,playback_error:{code:'playback_error'}}});
  const html=renderToStaticMarkup(tree);
  assert.ok(html.includes(t('player.playbackError.playback_error')));
  const toggle=nodes(tree).find(node=>node.type==='button'&&node.props.title===t(state==='play'?'player.pause':'player.play'));
  assert.equal(toggle.props.disabled,false);
 }
 const confirmed=renderToStaticMarkup(render({status:{...status,state:'play',playback_error:{code:'playback_error'}}}));
 assert.ok(!confirmed.includes(t('player.playbackError.playback_error')));
 const unavailable=renderToStaticMarkup(render({health:{state:'unavailable',reason:'process_exited'},status:{...status,state:'stop',playback_error:{code:'playback_error'}}}));
 assert.ok(!unavailable.includes(t('player.playbackError.playback_error')));
 const cleared=renderToStaticMarkup(render({status:{...status,state:'stop',playback_error:null}}));
 assert.ok(!cleared.includes(t('player.playbackError.playback_error')));
 for(const code of ['queue_uncertain','persistence_failed','format_unsupported','output_busy','output_unavailable']) {
  assert.ok(renderToStaticMarkup(render({status:{...status,state:'stop',playback_error:{code}}})).includes(t(`player.playbackError.${code}`)));
 }
});


test('DAC output diagnostics stop appearing after confirmed playback but remain actionable while stopped',async()=>{
 const {render,t}=await fixture();
 for(const code of ['output_busy','output_unavailable']) {
  const resumed=renderToStaticMarkup(render({status:{...status,state:'play',playback_error:{code}}}));
  assert.ok(!resumed.includes(t(`player.playbackError.${code}`)));
  const stopped=renderToStaticMarkup(render({status:{...status,state:'stop',playback_error:{code}}}));
  assert.ok(stopped.includes(t(`player.playbackError.${code}`)));
 }
});

test('Radio Direct refusals use a cause-neutral station message; Local and Plex keep their format notice',async()=>{
 for(const language of ['en','pt-BR']) {
  const {render,t}=await fixture(language);
  for(const kind of ['radio','local','plex']) {
   const tree=render({status:{...status,state:'stop',current_media:{kind},playback_error:{code:'format_unsupported'}}});
   const expected=kind==='radio'?'radio.directError':'player.playbackError.format_unsupported';
   assert.ok(renderToStaticMarkup(tree).includes(t(expected)));
   if(kind==='radio')assert.ok(!renderToStaticMarkup(tree).includes(t('player.playbackError.format_unsupported')));
  }
 }
});
