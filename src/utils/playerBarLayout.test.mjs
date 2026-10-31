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
import { routeConditions } from "./routeConditions.ts";
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
  const dependencies = { react: { ...React, default: React, __esModule: true }, "react/jsx-runtime": jsx, "lucide-react": icons, "react-i18next": { useTranslation: () => ({ t }) }, "../utils/signalPath": {...directVolumeHelpers, signalPathInformation, requiresDeviceVolumeControl }, "../utils/routeConditions": { routeConditions } };
  const SignalPath = compile("SignalPath", dependencies);
  const SourceAudioBadges = compile("SourceAudioBadges", { ...dependencies, "../utils/sourceAudio": { sourceAudioBadge, albumAudioBadge } });
  const hooks = { ...React, useState: initial => [initial, () => {}], useRef: initial => ({ current: initial }), useEffect() {} };
  const controlled = { ...dependencies,
    "./SleepTimerControl": { SleepTimerControl: () => null },
    react: { ...hooks, default: hooks, __esModule: true },
    "./SignalPath": { SignalPath }, "./PlexImage": { PlexImage: () => null },
    "./SourceAudioBadges": { SourceAudioBadges }, "../utils/playerNavigation": { currentMediaKey },
    "../utils/frontendDiagnostics": { logError(...args) { if (!sharedMuteError) assert.fail("Unexpected diagnostic"); diagnostics.push(args); } },
    "../services/audio": { audioService: Object.fromEntries(["togglePlay", "next", "previous", "seek", "setVolume", "setDirectMute", "setSharedMute", "changePlaybackOption"].map(name => [name, async (...args) => { calls.push([name, ...args]); if (name === "setSharedMute" && sharedMuteError) throw sharedMuteError; }])) },
  };
  const PlaybackOptionsControls = compile("PlaybackOptionsControls", controlled);
  const PlayerBar = compile("PlayerBar", { ...controlled, "./PlaybackOptionsControls": { PlaybackOptionsControls } });
  const props = { status, health: { state: "available" }, route: { mode: "direct", endpoint: "hw:Fixture,0" }, isQueueOpen: false,
    onNavigateToAlbum: () => calls.push(["album"]), onNavigateToArtist: () => calls.push(["artist"]), onToggleQueue: () => calls.push(["queue"]), onOpenNowPlaying: () => calls.push(["now-playing"]) };
  return { t, calls, diagnostics, props, SignalPath, PlaybackOptionsControls, render: changes => PlayerBar({ ...props, ...changes }) };
}

// Simulates the CSS tracks emitted by the actual component at the default
// Tailwind spacing scale (4px). This is geometry coverage, not a browser layout.
function geometry(tree, width) {
  const grid = /grid-cols-\[minmax\(0,1fr\)_clamp\((\d+)px,(\d+)vw,(\d+)px\)_minmax\(0,1fr\)\]/.exec(tree.props.className);
  assert.ok(grid, "Equal 1fr side tracks must surround the center");
  const padding = Number(/px-(\d+)/.exec(tree.props.className)[1]) * 4;
  const gap = Number(/gap-(\d+)/.exec(tree.props.className)[1]) * 4;
  const centerWidth = Math.max(Number(grid[1]), Math.min(width * Number(grid[2]) / 100, Number(grid[3])));
  const sideWidth = (width - 2 * padding - 2 * gap - centerWidth) / 2;
  const centerLeft = padding + sideWidth + gap;
  return { sideWidth, centerWidth, centerLeft, center: centerLeft + centerWidth / 2, padding, gap };
}

test("three regions center playback independently of side content at 1024, 1366 and 1920px", async () => {
  const { render, SignalPath, PlaybackOptionsControls, props } = await fixture();
  const offCenter = new Map();
  for (const width of [1024, 1366, 1920, 2560]) {
    for (const long of [false, true]) for (const mode of ["shared", "direct"]) for (const kind of ["local", "plex"]) {
      const tree = render({
        status: { ...status, title: long ? status.title : "Track", artist: long ? status.artist : "Artist", album: long ? status.album : "Album", current_media: { kind } },
        route: { mode, endpoint: mode === "shared" ? "default" : "hw:Fixture,0" },
        onOpenNowPlaying: long ? props.onOpenNowPlaying : undefined,
      });
      assert.match(tree.props.className, /h-28 shrink-0/);
      const regions = React.Children.toArray(tree.props.children);
      assert.deepEqual(regions.map(region => region.props["data-player-area"]), ["left", "playback", "actions"]);
      assert.ok(regions.every(region => region.props.className.includes("min-w-0")));
      const left = React.Children.toArray(regions[0].props.children);
      assert.deepEqual(left.map(region => region.props["data-player-area"]), ["metadata"]);
      assert.equal(nodes(left[0]).filter(node => node.type === SignalPath).length, 0);
      assert.equal(nodes(regions[0]).filter(node => node.type === SignalPath).length, 0);
      const signal = nodes(regions[2]).find(node => node.props["data-player-area"] === "signal");
      assert.ok(signal); assert.equal(nodes(signal).find(node => node.type === SignalPath).props.compact, true);
      assert.equal(nodes(tree).filter(node => node.type === SignalPath).length, 1);
      const layout = geometry(tree, width);
      assert.equal(layout.center, width / 2);
      assert.match(regions[1].props.className, /items-center/);
      const controls = nodes(regions[1]).find(node => node.props["data-player-controls"] !== undefined);
      assert.ok(controls);
      const children = React.Children.toArray(controls.props.children);
      const buttons = children.filter(node => node.type === "button");
      assert.equal(buttons.length, 3);
      assert.equal(children.length, 4);
      const optionsNode = nodes(children[3]).find(node => node.type === PlaybackOptionsControls);
      assert.ok(optionsNode);
      const options = PlaybackOptionsControls(optionsNode.props);
      const optionButtons = nodes(options).filter(node => node.type === "button");
      assert.equal(optionButtons.length, 2);
      const optionsWidth = optionButtons.reduce((width, button) => width + iconSizeOf(button) + 2 * button.props.style.padding, 0) + 8;
      // Derive tracks and item positions from the emitted grid/classes, including
      // real option button widths. Do not equate the group center with Play.
      assert.match(controls.props.className, /grid w-full/);
      const tracks = /grid-cols-\[minmax\(0,1fr\)_(\d+)px_(\d+)px_(\d+)px_minmax\(0,1fr\)\]/.exec(controls.props.className);
      assert.ok(tracks, "equal flexible side tracks must flank the transport");
      const gap = Number(/gap-([\d.]+)/.exec(controls.props.className)[1]) * 4;
      const fixed = tracks.slice(1).map(Number);
      const side = (layout.centerWidth - fixed.reduce((a,b) => a+b,0) - 4 * gap) / 2;
      const widths = [side, ...fixed, side];
      const bounds = node => {
        const column = Number(/col-start-(\d+)/.exec(node.props.className)[1]) - 1;
        const left = layout.centerLeft + widths.slice(0,column).reduce((a,b) => a+b,0) + column * gap;
        return { left, right: left + widths[column], width: widths[column] };
      };
      const previous = bounds(buttons[0]), play = bounds(buttons[1]), next = bounds(buttons[2]), extras = bounds(children[3]);
      const playWidth = Number(/\bw-(\d+)\b/.exec(buttons[1].props.className)[1]) * 4;
      assert.equal(play.width, playWidth);
      for (const [button, track] of [[buttons[0], previous], [buttons[2], next]]) {
        assert.match(button.props.className, /justify-self-center/);
        assert.ok(iconSizeOf(button) <= track.width);
      }
      assert.equal(iconSizeOf(buttons[0]), iconSizeOf(buttons[2]));
      const playCenter = play.left + playWidth / 2;
      if (Math.abs(playCenter - width / 2) > 1e-9) offCenter.set(width, { actual: playCenter, expected: width / 2 });
      assert.ok(Math.abs((playCenter - (previous.left + previous.width/2)) - ((next.left + next.width/2) - playCenter)) < 1e-9, "Previous/Next remain symmetric around Play");
      assert.ok(previous.right <= play.left && play.right <= next.left && next.right <= extras.left, "transport and options never overlap");
      assert.match(children[3].props.className, /justify-start/);
      assert.ok(optionsWidth <= extras.width, "both real repeat/random buttons fit their side track");
      assert.ok(extras.left + optionsWidth <= layout.centerLeft + layout.centerWidth, "options stay inside the central region");
      assert.doesNotMatch(controls.props.className + buttons.map(button => button.props.className).join(" "), /absolute|translate-|left-|right-/);
      const progress = nodes(regions[1]).find(node => node.type === "div" && node.props.className.startsWith("w-full flex items-center gap-3"));
      assert.ok(progress);
      const progressChildren = React.Children.toArray(progress.props.children);
      assert.match(progressChildren[0].props.className, /w-8/);
      assert.match(progressChildren[2].props.className, /w-8/);
      const seekWidth = layout.centerWidth - 2 * 32 - 2 * 12;
      const seekLeft = layout.centerLeft + 32 + 12;
      assert.equal(seekLeft + seekWidth / 2, width / 2);
      assert.ok(seekWidth >= 150);
      const signalMaximum = Number(/max-w-\[(\d+)px\]/.exec(signal.props.className)[1]);
      assert.match(signal.props.className, /min-w-0.*flex-1/);
      // Only the secondary indicator yields to fixed-size volume/buttons.
      const actionCount = props.onOpenNowPlaying && long ? 2 : 1;
      const fixedActionsWidth = 122 + actionCount * 39 + (actionCount + 1) * 8;
      const signalWidth = Math.min(signalMaximum, layout.sideWidth - fixedActionsWidth);
      const cover = nodes(left[0]).find(node => node.props.className?.includes("w-[76px]"));
      assert.ok(cover); assert.match(cover.props.className, /h-\[76px\].*shrink-0/);
      assert.ok(layout.sideWidth - 76 - 8 - 8 >= 200, "Metadata uses the left region without a Signal Path competing for width");
      assert.ok(signalWidth >= 110 && signalWidth <= 190);
      assert.ok(layout.sideWidth >= fixedActionsWidth + signalWidth, "Signal, volume and action buttons fit within the right region");
      const volume = React.Children.toArray(regions[2].props.children).find(node => node.props.className?.includes("items-end"));
      assert.match(volume.props.className, /shrink-0/);
      assert.equal(2 * layout.padding + 2 * layout.gap + 2 * layout.sideWidth + layout.centerWidth, width);
      assert.doesNotMatch(renderToStaticMarkup(tree), /overflow-x-auto|overflow-x-scroll/);
      if (long) for (const value of [status.title, status.artist, status.album]) assert.ok(nodes(left[0]).some(node => node.props.title === value && node.props.className.includes("truncate")));
    }
  }
  assert.deepEqual([...offCenter], [], "actual Play/Pause must remain centered including repeat/random controls");
});

test("queue drawer and Save feedback clear the new 112px player without shifting unrelated layouts", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const queue = source("QueueDrawer");
  assert.match(app, /fixed bottom-28 right-6/);
  assert.match(queue, /className="np-queue-drawer"/);
  const css = readFileSync(new URL("../components/nowPlaying.css", import.meta.url), "utf8");
  const drawer = css.match(/\.np-queue-drawer\s*\{([^}]+)\}/)[1];
  for (const rule of [/position:\s*fixed/, /top:\s*0/, /right:\s*0/, /bottom:\s*112px/]) assert.match(drawer, rule);
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
  const playback = nodes(tree).find(node => node.props["data-player-area"] === "playback");
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
  assert.ok(html.includes(t("signalPath.assessment.partial")));assert.match(html,/aria-haspopup="dialog"/);
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

test('retained generic MPD errors stay out of the player while explicit playback remains available',async()=>{
 const {render,t}=await fixture();
 for(const state of ['play','pause','stop']) {
  const tree=render({status:{...status,state,playback_error:{code:'playback_error'}}});
  const html=renderToStaticMarkup(tree);
  assert.ok(!html.includes(t('player.playbackError.playback_error')));
  const toggle=nodes(tree).find(node=>node.type==='button'&&node.props.title===t(state==='play'?'player.pause':'player.play'));
  assert.equal(toggle.props.disabled,false);
 }
 for(const code of ['queue_uncertain','persistence_failed','format_unsupported']) {
  assert.ok(renderToStaticMarkup(render({status:{...status,playback_error:{code}}})).includes(t(`player.playbackError.${code}`)));
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
