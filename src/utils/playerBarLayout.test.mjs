import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import ts from "typescript";
import i18next from "i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { signalPathInformation } from "./signalPath.ts";

const resource = language => JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url)));
const source = name => readFileSync(new URL(`../components/${name}.tsx`, import.meta.url), "utf8");
const status = { state: "play", title: "A very long track title ".repeat(12), artist: "An artist ".repeat(10), album: "An album ".repeat(10), audio_format: "192000:24:2", current_media: { kind: "local" }, elapsed: 12, duration: 180, volume: { backend: "mpd_software", available: true, writable: true, value: 50, muted: false } };
const nodes = element => React.isValidElement(element) ? [element, ...nodes(element.props.children)] : Array.isArray(element) ? element.flatMap(nodes) : [];
function compile(name, dependencies) {
  const exports = {};
  const js = ts.transpileModule(source(name), { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, exports);
  return exports[name];
}
async function fixture(language = "en") {
  const i18n = i18next.createInstance();
  await i18n.init({ lng: language, resources: { [language]: { translation: resource(language) } }, interpolation: { escapeValue: false } });
  const t = i18n.t.bind(i18n), calls = [];
  const dependencies = { react: { ...React, default: React, __esModule: true }, "react/jsx-runtime": jsx, "lucide-react": icons, "react-i18next": { useTranslation: () => ({ t }) }, "../utils/signalPath": { signalPathInformation } };
  const SignalPath = compile("SignalPath", dependencies);
  const hooks = { ...React, useState: initial => [initial, () => {}], useRef: initial => ({ current: initial }), useEffect() {} };
  const PlayerBar = compile("PlayerBar", { ...dependencies,
    react: { ...hooks, default: hooks, __esModule: true },
    "./SignalPath": { SignalPath }, "./PlexImage": { PlexImage: () => null },
    "../utils/frontendDiagnostics": { logError() { assert.fail("Unexpected diagnostic"); } },
    "../services/audio": { audioService: Object.fromEntries(["togglePlay", "next", "previous", "seek", "setVolume"].map(name => [name, async (...args) => { calls.push([name, ...args]); }])) },
  });
  const props = { status, health: { state: "available" }, route: { mode: "direct", endpoint: "hw:Fixture,0" }, isQueueOpen: false,
    onNavigateToAlbum: () => calls.push(["album"]), onNavigateToArtist: () => calls.push(["artist"]), onToggleQueue: () => calls.push(["queue"]), onOpenNowPlaying: () => calls.push(["now-playing"]) };
  return { t, calls, props, SignalPath, render: changes => PlayerBar({ ...props, ...changes }) };
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
  const { render, SignalPath, props } = await fixture();
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
      const controls = nodes(regions[1]).find(node => node.type === "div" && node.props.className === "flex items-center gap-7");
      assert.ok(controls);
      const buttons = React.Children.toArray(controls.props.children);
      assert.equal(buttons.length, 3);
      // Previous/Next are equal-width icons. The middle Play/Pause button and
      // its symmetric gaps are centered by the playback region's items-center.
      const iconSize = button => React.Children.toArray(button.props.children)[0].props.size;
      assert.equal(iconSize(buttons[0]), iconSize(buttons[2]));
      const controlsWidth = iconSize(buttons[0]) + 40 + iconSize(buttons[2]) + 2 * 28;
      assert.equal(layout.centerLeft + (layout.centerWidth - controlsWidth) / 2 + controlsWidth / 2, width / 2);
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
});

test("queue drawer and Save feedback clear the new 112px player without shifting unrelated layouts", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const queue = source("QueueDrawer");
  assert.match(app, /fixed bottom-28 right-6/);
  assert.match(queue, /fixed top-0 right-0 bottom-28/);
  assert.doesNotMatch(app, /bottom-24/);
  assert.doesNotMatch(queue, /bottom-24/);
});

test("discreet two-line signal shows mode, relevant format and accessible full information", async () => {
  for (const language of ["en", "pt-BR"]) {
    const { render, t } = await fixture(language);
    for (const mode of ["shared", "direct"]) for (const kind of ["local", "plex"]) {
      const html = renderToStaticMarkup(render({ route: { mode, endpoint: mode === "shared" ? "default" : "hw:Fixture,0" }, status: { ...status, current_media: { kind } } }));
      assert.ok(html.includes(t("signalPath.title")));
      assert.ok(html.includes(t(`signalPath.${mode}`))); assert.ok(html.includes(t(`player.source.${kind}`)));
      assert.match(html, /192 kHz/); assert.match(html, /24-bit/);
      assert.ok(html.includes(t("signalPath.help")));
      assert.match(html, /aria-description=/); assert.match(html, /group-focus-within\/compact-signal:block/);
      assert.match(html, /flex min-w-0 flex-col gap-1/);
      assert.ok(html.includes(t("signalPath.pcm", { rate: 192, bits: 24, channels: 2 })));
      const tree = render({ route: { mode, endpoint: mode === "shared" ? "default" : "hw:Fixture,0" }, status: { ...status, current_media: { kind } } });
      const signalNode = nodes(tree).find(node => node.props.compact);
      const { SignalPath } = await fixture(language);
      const compact = SignalPath(signalNode.props);
      const visible = React.Children.toArray(compact.props.children).filter(node => node.props.role !== "tooltip");
      assert.equal(visible.length, 2);
      assert.equal(visible[0].props.children, t(`signalPath.${mode}`));
      assert.equal(visible[1].props.children, t("signalPath.pcmCompact", { rate: 192, bits: 24 }));
      assert.doesNotMatch(compact.props.className, /bg-|border|rounded|py-/);

    }
  }
});

test("navigation, playback, seek, volume and actions retain their handlers", async () => {
  const { render, calls, t } = await fixture();
  const tree = render(), all = nodes(tree);
  const click = async title => { const button = all.find(node => node.type === "button" && node.props.title === title); assert.ok(button); await button.props.onClick(); };
  await click(status.title); await click(status.album); await click(status.artist);
  await click(t("player.previous")); await click(t("player.pause")); await click(t("player.next"));
  const ranges = all.filter(node => node.type === "input" && node.props.type === "range");
  assert.equal(ranges.length, 2);
  await ranges[0].props.onMouseUp({ target: { value: "42.5" } });
  await ranges[1].props.onChange({ target: { value: "25" } });
  await click(t("player.mute")); await click(t("player.openNowPlaying")); await click(t("player.queue"));
  assert.deepEqual(calls, [["album"], ["album"], ["artist"], ["previous"], ["togglePlay"], ["next"], ["seek", 42.5], ["setVolume", 25], ["setVolume", 0], ["now-playing"], ["queue"]]);
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
  assert.ok(html.includes(t("signalPath.unknownRoute")));
  assert.ok(html.includes(t("signalPath.unknownFormat")));
  assert.doesNotMatch(html, /unrecognized private input/);
  const unmute = nodes(tree).find(node => node.type === "button" && node.props.title === t("player.unmute"));
  assert.ok(unmute); await unmute.props.onClick();
  assert.deepEqual(calls, [["setVolume", 100]]);
});
