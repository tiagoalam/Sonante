import * as frontendDiagnostics from "./frontendDiagnostics.ts";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import { audioRestartExpected, localFoldersOnlyChanged } from "./audioConfigChange.ts";
import { createDeviceAvailabilityQuery, newPhysicalOutputChoice, shouldCheckContextualDevice } from "./deviceAvailability.ts";

const source = name => readFileSync(new URL(`../components/${name}.tsx`, import.meta.url), "utf8");
const t = key => key;
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

// A small hook harness runs the actual components and their event handlers,
// without starting Tauri, a browser, MPD or timers. Effects retain React's deps.
function component(name, dependencies = {}) {
  let index = 0, sequence = 0;
  const slots = [], pending = [];
  const hooks = {
    ...React,
    useState(initial) {
      const key = index++;
      if (!(key in slots)) slots[key] = typeof initial === "function" ? initial() : initial;
      return [slots[key], value => { slots[key] = typeof value === "function" ? value(slots[key]) : value; }];
    },
    useRef(initial) { const key = index++; return slots[key] ??= { current: initial }; },
    useId() { const key = index++; return slots[key] ??= `fixture-${sequence++}`; },
    useEffect(effect, deps) {
      const key = index++, old = slots[key];
      if (!old || !deps || deps.some((value, i) => !Object.is(value, old.deps[i]))) {
        pending.push(() => { old?.cleanup?.(); slots[key] = { deps, cleanup: effect() }; });
      }
    },
  };
  const defaults = {
    react: { ...hooks, default: hooks, __esModule: true }, "react/jsx-runtime": jsx,
    "../utils/frontendDiagnostics": frontendDiagnostics,
    "lucide-react": icons,
    "react-i18next": { useTranslation: () => ({ t, i18n: { language: "en", changeLanguage() {} } }) },
  };
  const js = ts.transpileModule(source(name), { compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  const exports = {};
  new Function("require", "exports", js)(id => {
    if (id in dependencies) return dependencies[id];
    if (id in defaults) return defaults[id];
    throw new Error(`Unexpected import: ${id}`);
  }, exports);
  return {
    type: exports[name],
    render(props) { index = 0; return exports[name](props); },
    effects() { pending.splice(0).forEach(run => run()); },
    dispose() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}
function nodes(tree) {
  if (!React.isValidElement(tree)) return Array.isArray(tree) ? tree.flatMap(nodes) : [];
  const children = tree.type?.name === "SettingsPages"
    ? [tree.props.general, tree.props.audio, tree.props.local, tree.props.plex] : tree.props.children;
  return [tree, ...nodes(children)];
}
const find = (tree, predicate) => { const result = nodes(tree).find(predicate); assert.ok(result, "Expected control exists"); return result; };
const text = tree => React.isValidElement(tree) ? text(tree.props.children) : Array.isArray(tree) ? tree.map(text).join("") : typeof tree === "string" || typeof tree === "number" ? String(tree) : "";
const button = (tree, key) => find(tree, node => node.type === "button" && text(node) === key);
const select = (tree, suffix) => find(tree, node => node.type === "select" && node.props.id?.endsWith(suffix));
const pageProps = tree => find(tree, node => node.type?.name === "SettingsPages").props;
const base = {
  first_run: false, audio_output_type: "alsa", alsa_device: "hw:Fixture,0", local_folders: ["/fixture"],
  online_artwork_enabled: false, plex_token: "", plex_url: "", playback_mode: "http",
  local_mount_path: "", remote_share_path: "", dop_enabled: false, audio_buffer_size_kb: 8192, replay_gain: "off",
};
async function modal(overrides = {}) {
  const calls = { config: 0, devices: 0, save: [], started: [], saved: [], accepted: [], local: [], closed: 0, finished: 0, availability: 0, scans: 0, pins: 0 };
  const query = { resolve: null, requests: [] };
  const configService = {
    async getConfig() { calls.config++; return structuredClone(overrides.config ?? base); },
    async getAudioDevices() { calls.devices++; return overrides.devices ?? [{ id: base.alsa_device, name: "Fixture DAC" }]; },
    async checkDirectDeviceAvailability(id) { calls.availability++; return new Promise((resolve, reject) => { query.resolve = resolve; query.requests.push({ id, resolve, reject }); }); },
    async saveConfig(config) { calls.save.push(config); return overrides.save ? overrides.save(config) : overrides.response ?? { status: "completed" }; },
  };
  const audioService = { async pickDirectory() { return "/new-fixture"; }, async checkLocalLibrary() { calls.scans++; return false; }, async rescanLibrary() { calls.scans++; } };
  const plexService = { async getServers() { return []; }, async createPin() { calls.pins++; return { id: 7, code: "fixture", auth_url: "https://example.invalid" }; }, async openExternalUrl() {} };
  const h = component("SettingsModal", {
    "./SettingsPages": { SettingsPages: function SettingsPages() {} },
    "./AudioSignalDiagram": { AudioSignalDiagram: function AudioSignalDiagram() {} },
    "../services/config": { configService }, "../services/audio": { audioService }, "../services/plex": { plexService },
    "../utils/audioConfigChange": { audioRestartExpected, localFoldersOnlyChanged },
    "../utils/deviceAvailability": { createDeviceAvailabilityQuery, newPhysicalOutputChoice, shouldCheckContextualDevice },
    "../utils/localAlbumCatalog": {
      async refreshCatalogAfterSuccessfulCheck(read, notify) { const changed = await read(); if (changed) notify(); return changed; },
      async refreshCatalogAfterSuccessfulRescan(read, notify) { await read(); notify(); },
    },
  });
  const props = {
    onClose() { calls.closed++; }, onSaveStarted(restart) { calls.started.push(restart); return true; },
    onSaved(restart) { calls.saved.push(restart); }, onSaveFinished() { calls.finished++; },
    onSaveAccepted(operation) { calls.accepted.push(operation); },
    onStartLocalFoldersSave(config) { calls.local.push(config); return true; },
    onLibraryRescanned() {}, isLibraryUpdating: false, ...overrides.props,
  };
  let tree;
  const render = () => { tree = h.render(props); h.effects(); return tree; };
  assert.equal(render(), null);
  await flush(); render();
  return { h, calls, query, render, tree: () => tree };
}

// Only DOM focus is used by the mount effect; no real UI is opened.
const previousDocument = globalThis.document;
const previousHTMLElement = globalThis.HTMLElement;
globalThis.document = { activeElement: null };
globalThis.HTMLElement = class {};
test.after(() => { globalThis.document = previousDocument; globalThis.HTMLElement = previousHTMLElement; });

test("four mounted pages share one scroll panel and support keyboard navigation", () => {
  const h = component("SettingsPages");
  const contents = Object.fromEntries(["general", "audio", "local", "plex"].map(id => [id, React.createElement("input", { defaultValue: id })]));
  let tree = h.render(contents);
  const panels = () => nodes(tree).filter(node => node.props.role === "tabpanel");
  const tabs = () => nodes(tree).filter(node => node.props.role === "tab");
  assert.equal(panels().length, 4);
  assert.deepEqual(panels().map(node => node.props.hidden), [false, true, true, true]);
  const retained = panels().map(node => node.props.children[1]);
  for (let i = 0; i < 4; i++) {
    tabs()[i].props.onClick(); tree = h.render(contents);
    assert.deepEqual(panels().map(node => node.props.hidden), [0, 1, 2, 3].map(n => n !== i));
    assert.deepEqual(panels().map(node => node.props.children[1]), retained);
    assert.equal(tabs()[i].props.tabIndex, 0);
    assert.equal(panels()[i].props["aria-labelledby"], tabs()[i].props.id);
  }
  let focused = false, prevented = false;
  tabs()[0].props.ref({ focus() { focused = true; } });
  tabs()[3].props.onKeyDown({ key: "ArrowDown", preventDefault() { prevented = true; } });
  tree = h.render(contents);
  assert.equal(panels()[0].props.hidden, false);
  assert.equal(focused && prevented, true);
  tabs()[0].props.onKeyDown({ key: "End", preventDefault() {} }); tree = h.render(contents);
  assert.equal(panels()[3].props.hidden, false);
  tabs()[3].props.onKeyDown({ key: "Home", preventDefault() {} }); tree = h.render(contents);
  assert.equal(panels()[0].props.hidden, false);
  const scroll = find(tree, node => node.props.className?.includes("overflow-y-auto"));
  scroll.props.ref.current = { scrollTop: 200 };
  tabs()[2].props.onClick(); assert.equal(scroll.props.ref.current.scrollTop, 0);
  assert.equal(nodes(tree).filter(node => node.props.className?.includes("overflow-y-auto")).length, 1);
  h.dispose();
});

test("audio draft survives page changes and saving retains existing controls and restart semantics", async () => {
  const f = await modal();
  const pages = component("SettingsPages");
  let tree = f.tree();
  const options = suffix => nodes(select(tree, suffix)).filter(node => node.type === "option").map(node => node.props.value);
  assert.deepEqual(options("-buffer"), [4096, 8192, 16384, 32768]);
  assert.deepEqual(options("-replaygain"), ["off", "album", "track"]);
  select(tree, "-buffer").props.onChange({ target: { value: "32768" } }); tree = f.render();
  select(tree, "-replaygain").props.onChange({ target: { value: "album" } }); tree = f.render();
  find(tree, node => node.type === "input" && node.props["aria-label"] === "settings.dop").props.onChange({ target: { checked: true } }); tree = f.render();
  for (const id of ["plex", "local", "general", "audio"]) {
    const p = pages.render(pageProps(tree));
    button(p, `settings.pages.${id}`).props.onClick();
    tree = f.render();
  }
  assert.equal(select(tree, "-buffer").props.value, 32768);
  assert.equal(select(tree, "-replaygain").props.value, "album");
  assert.equal(find(tree, node => node.type === "input" && node.props["aria-label"] === "settings.dop").props.checked, true);
  button(tree, "settings.save").props.onClick(); await flush(); f.render();
  assert.equal(f.calls.save.length, 1);
  assert.equal(f.calls.save[0].dop_enabled, true);
  assert.deepEqual(f.calls.started, [true]); assert.deepEqual(f.calls.saved, [true]);
  assert.equal(f.calls.finished, 1); assert.equal(f.calls.closed, 1);
  assert.equal(f.calls.config, 1); assert.equal(f.calls.devices, 1);
  pages.dispose(); f.h.dispose();
});

test("mode controls update the planned diagram immediately and keep DoP semantics", async () => {
  const f = await modal();
  let tree = f.tree();
  const diagram = () => find(tree, node => node.type?.name === "AudioSignalDiagram").props;
  assert.equal(diagram().mode, "direct");
  find(tree, node => node.type === "button" && text(node).includes("settings.pipewireMode")).props.onClick(); tree = f.render();
  assert.equal(diagram().mode, "shared");
  assert.equal(find(tree, node => node.type === "input" && node.props["aria-label"] === "settings.dop").props.disabled, true);
  find(tree, node => node.type === "button" && text(node).includes("settings.alsaMode")).props.onClick(); tree = f.render();
  select(tree, "-device").props.onChange({ target: { value: "default" } }); tree = f.render();
  assert.equal(diagram().mode, "shared", "matches supervisor's default endpoint classification");
  select(tree, "-device").props.onChange({ target: { value: "hw:Another,0" } }); tree = f.render();
  assert.equal(diagram().mode, "direct"); assert.equal(diagram().device, "hw:Another,0");
  assert.equal(f.calls.availability, 0, "no automatic device probes on page/mode changes");
  f.h.dispose();
});

test("Cancel and X discard unsaved edits; reopening loads persisted configuration", async () => {
  for (const close of ["cancel", "X"]) {
    const f = await modal();
    select(f.tree(), "-buffer").props.onChange({ target: { value: "16384" } }); const tree = f.render();
    if (close === "cancel") button(tree, "settings.cancel").props.onClick();
    else find(tree, node => node.type === "button" && node.props["aria-label"] === "settings.close").props.onClick();
    assert.equal(f.calls.closed, 1); assert.equal(f.calls.save.length, 0);
    f.h.dispose();
    const reopened = await modal(); assert.equal(select(reopened.tree(), "-buffer").props.value, base.audio_buffer_size_kb); reopened.h.dispose();
  }
});

test("folders-only Save and accepted Save keep existing ownership transfer", async () => {
  const local = await modal();
  await button(local.tree(), "settings.addFolder").props.onClick(); local.render();
  await button(local.tree(), "settings.save").props.onClick();
  assert.deepEqual(local.calls.local[0].local_folders, ["/fixture", "/new-fixture"]);
  assert.equal(local.calls.save.length, 0); assert.equal(local.calls.closed, 1); local.h.dispose();
  const operation = { id: "fixture-operation", phase: "preparing", error: null };
  const accepted = await modal({ response: { status: "accepted", operation } });
  await button(accepted.tree(), "settings.save").props.onClick();
  assert.deepEqual(accepted.calls.accepted, [operation]); assert.equal(accepted.calls.saved.length, 0);
  assert.equal(accepted.calls.finished, 0); assert.equal(accepted.calls.closed, 1); accepted.h.dispose();
});

test("explicit availability request survives page changes without another request", async () => {
  const f = await modal(); const pages = component("SettingsPages");
  const request = button(f.tree(), "settings.deviceAvailability.check").props.onClick();
  f.render();
  for (const id of ["local", "plex", "audio"]) {
    button(pages.render(pageProps(f.tree())), `settings.pages.${id}`).props.onClick(); f.render();
  }
  assert.equal(f.calls.availability, 1);
  f.query.resolve([{ id: base.alsa_device, state: "available" }]); await request; await flush();
  assert.ok(nodes(f.render()).some(node => node.props.role === "status" && text(node).includes("settings.deviceAvailability.available")));
  pages.dispose(); f.h.dispose();
});

test("signal diagrams render ordered routes and clearly label a draft, without inspecting audio", () => {
  const h = component("AudioSignalDiagram");
  for (const mode of ["shared", "direct"]) {
    const tree = h.render({ mode, device: "hw:Fixture,0" });
    const markup = renderToStaticMarkup(tree);
    const labels = nodes(tree).filter(node => node.type === "li").map(text);
    assert.equal(labels.length, mode === "shared" ? 5 : 4);
    assert.ok(labels[0].includes("settings.signal.source"));
    assert.ok(labels[1].includes("settings.signal.mpd"));
    assert.ok(labels[2].includes(mode === "shared" ? "default" : "hw:Fixture,0"));
    assert.equal(markup.includes("settings.signal.system"), mode === "shared");
    assert.ok(markup.includes("settings.signal.notice"));
    assert.doesNotMatch(markup, /<(a|input|audio)\b|href=/);
    assert.equal(nodes(tree).filter(node => node.type === "button").length, mode === "shared" ? 5 : 4);
    assert.equal(find(tree, node => node.type === "details").props.open, undefined);
  }
  h.dispose();
});

test("Plex PIN polling stays mounted across page changes and is cleaned up once", async () => {
  const oldWindow = globalThis.window, oldClear = globalThis.clearInterval;
  const started = [], cleared = [];
  globalThis.window = { setInterval(callback, delay) { started.push({ callback, delay }); return 1; } };
  globalThis.clearInterval = id => cleared.push(id);
  let f, pages;
  try {
    f = await modal(); pages = component("SettingsPages");
    await button(f.tree(), "settings.connectPlex").props.onClick(); f.render();
    assert.equal(started.length, 1);
    for (const id of ["general", "audio", "local", "plex", "audio", "plex"]) {
      button(pages.render(pageProps(f.tree())), `settings.pages.${id}`).props.onClick(); f.render();
    }
    assert.equal(f.calls.pins, 1); assert.equal(started.length, 1); assert.equal(cleared.length, 0);
    assert.ok(nodes(f.tree()).some(node => text(node).includes("settings.waitingBrowser")));
    f.h.dispose(); f = null;
    assert.deepEqual(cleared, [1]);
  } finally {
    f?.h.dispose(); pages?.dispose(); globalThis.window = oldWindow; globalThis.clearInterval = oldClear;
  }
});

test("pending Save fences duplicate submissions and disables global closing controls", async () => {
  let complete;
  const f = await modal({ save: () => new Promise(resolve => { complete = resolve; }) });
  const save = button(f.tree(), "settings.save");
  const pending = save.props.onClick(); f.render();
  await save.props.onClick();
  assert.equal(f.calls.save.length, 1);
  assert.equal(button(f.tree(), "settings.cancel").props.disabled, true);
  assert.equal(button(f.tree(), "settings.savingSettings").props.disabled, true);
  assert.equal(find(f.tree(), node => node.props["aria-label"] === "settings.close").props.disabled, true);
  complete({ status: "completed" }); await pending; f.render();
  assert.equal(f.calls.finished, 1); assert.equal(f.calls.closed, 1); f.h.dispose();
});

test("Save failure preserves sanitized diagnostics, reloads confirmed config and never announces success", async () => {
  const alerts = [], oldAlert = globalThis.alert, oldError = console.error;
  globalThis.alert = message => alerts.push(message); console.error = () => {};
  let f;
  try {
    f = await modal({ save: async () => { throw "fixture failure"; } });
    select(f.tree(), "-buffer").props.onChange({ target: { value: "32768" } }); f.render();
    await button(f.tree(), "settings.save").props.onClick(); f.render();
    assert.equal(f.calls.config, 2); assert.equal(f.calls.closed, 0); assert.equal(f.calls.saved.length, 0);
    assert.equal(f.calls.finished, 1); assert.equal(select(f.tree(), "-buffer").props.value, base.audio_buffer_size_kb);
    assert.deepEqual(alerts, ["settings.outputConfigChangeError\n\nsettings.backendReportedStep\nfixture failure"]);
  } finally { f?.h.dispose(); globalThis.alert = oldAlert; console.error = oldError; }
});

test("dialog focus trap ignores hidden pages, disabled controls and inactive tabs", async () => {
  const f = await modal(); const dialog = find(f.tree(), node => node.props.role === "dialog");
  assert.equal(dialog.props["aria-modal"], "true");
  assert.ok(nodes(f.tree()).some(node => node.type === "h2" && node.props.id === dialog.props["aria-labelledby"]));
  const focused = [];
  const control = (name, options = {}) => ({
    tabIndex: options.inactive ? -1 : 0, closest: () => options.hidden,
    hasAttribute: () => options.disabled, focus: () => focused.push(name),
  });
  const first = control("close"), last = control("save");
  dialog.props.ref.current = { querySelectorAll: () => [first, control("hidden", { hidden: true }),
    control("inactive", { inactive: true }), last, control("disabled", { disabled: true })] };
  let prevented = 0;
  document.activeElement = last;
  dialog.props.onKeyDown({ key: "Tab", shiftKey: false, preventDefault() { prevented++; } });
  document.activeElement = first;
  dialog.props.onKeyDown({ key: "Tab", shiftKey: true, preventDefault() { prevented++; } });
  assert.deepEqual(focused, ["close", "save"]); assert.equal(prevented, 2);
  document.activeElement = null; f.h.dispose();
});

const availableDevices = [base.alsa_device, "hw:Other,0", "hw:Third,0", "hw:Last,0", "null"].map(id => ({ id, name: id }));
const choose = (f, id) => { select(f.tree(), "-device").props.onChange({ target: { value: id } }); f.render(); };
const statusText = f => nodes(f.tree()).filter(node => node.props.role === "status").map(text).join(" ");

test("new enumerated physical selection checks once; opening, virtual choices and duplicate events do not probe", async () => {
  const f = await modal({ devices: availableDevices });
  assert.equal(f.calls.availability, 0); assert.ok(statusText(f).includes("settings.deviceAvailability.indeterminate"));
  choose(f, "hw:Other,0");
  assert.deepEqual(f.query.requests.map(request => request.id), ["hw:Other,0"]);
  assert.ok(statusText(f).includes("settings.deviceAvailability.checking"));
  assert.equal(button(f.tree(), "settings.save").props.disabled, true);
  choose(f, "hw:Other,0"); assert.equal(f.calls.availability, 1);
  f.query.requests[0].resolve([{ id: "hw:Other,0", state: "available" }]); await flush(); f.render();
  assert.ok(statusText(f).includes("settings.deviceAvailability.available"));
  choose(f, "null"); assert.equal(f.calls.availability, 1);
  choose(f, "hw:NotEnumerated,0"); assert.equal(f.calls.availability, 1);
  f.h.dispose();
});

test("rapid physical selection ignores stale results and requests only the latest queued endpoint", async () => {
  const f = await modal({ devices: availableDevices });
  choose(f, "hw:Other,0"); choose(f, "hw:Third,0"); choose(f, "hw:Last,0");
  assert.equal(f.calls.availability, 1);
  f.query.requests[0].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
  assert.deepEqual(f.query.requests.map(request => request.id), ["hw:Other,0", "hw:Last,0"]);
  assert.ok(!statusText(f).includes("settings.deviceAvailability.busy"));
  f.query.requests[1].resolve([{ id: "hw:Last,0", state: "available" }]); await flush(); f.render();
  assert.ok(statusText(f).includes("settings.deviceAvailability.available"));
  f.h.dispose();
});

test("Shared selection and closing discard pending checks; selecting the current device preserves ownership status", async () => {
  const f = await modal({ devices: availableDevices });
  choose(f, "hw:Other,0");
  find(f.tree(), node => node.type === "button" && text(node).includes("settings.pipewireMode")).props.onClick(); f.render();
  f.query.requests[0].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
  assert.ok(!nodes(f.tree()).some(node => node.props.role === "alert" && text(node).includes("settings.deviceAvailability.busyAction")));
  find(f.tree(), node => node.type === "button" && text(node).includes("settings.alsaMode")).props.onClick(); f.render();
  assert.equal(f.query.requests[1].id, "hw:Other,0");
  choose(f, base.alsa_device);
  f.query.requests[1].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
  f.query.requests[2].resolve([{ id: base.alsa_device, state: "current_sonante" }]); await flush(); f.render();
  assert.ok(statusText(f).includes("settings.deviceAvailability.indeterminate"));
  assert.ok(!statusText(f).includes("settings.deviceAvailability.current_sonante"), "configuration ownership does not prove an open PCM");
  choose(f, "hw:Other,0");
  button(f.tree(), "settings.cancel").props.onClick(); f.h.dispose();
  f.query.requests[3].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush();
  assert.equal(f.calls.save.length, 0); assert.equal(f.calls.closed, 1);
});

test("busy selection retries on Save and refuses only a freshly confirmed conflict", async () => {
  const f = await modal({ devices: availableDevices });
  choose(f, "hw:Other,0");
  f.query.requests[0].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
  assert.ok(nodes(f.tree()).some(node => node.props.role === "alert" && text(node).includes("settings.deviceAvailability.busyAction")));
  assert.equal(button(f.tree(), "settings.save").props.disabled, false, "old busy observation does not prevent a fresh check");
  const refused = button(f.tree(), "settings.save").props.onClick(); f.render();
  assert.equal(f.calls.save.length, 0); assert.equal(f.calls.started.length, 0);
  f.query.requests[1].resolve([{ id: "hw:Other,0", state: "busy" }]); await refused; await flush(); f.render();
  assert.equal(f.calls.save.length, 0); assert.equal(f.calls.started.length, 0); assert.equal(f.calls.closed, 0);
  const retry = button(f.tree(), "settings.save").props.onClick(); f.render();
  f.query.requests[2].resolve([{ id: "hw:Other,0", state: "available" }]); await retry; await flush(); f.render();
  assert.equal(f.calls.save.length, 1); assert.equal(f.calls.save[0].alsa_device, "hw:Other,0");
  assert.deepEqual(f.calls.started, [true]); f.h.dispose();
});

test("stale busy recheck during Save cannot authorize an outdated device choice", async () => {
  const f = await modal({ devices: availableDevices });
  choose(f, "hw:Other,0"); f.query.requests[0].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
  const save = button(f.tree(), "settings.save").props.onClick(); f.render();
  choose(f, "hw:Third,0"); await save;
  f.query.requests[1].resolve([{ id: "hw:Other,0", state: "available" }]); await flush(); f.render();
  assert.equal(f.calls.save.length, 0); assert.equal(f.calls.started.length, 0);
  f.query.requests[2].resolve([{ id: "hw:Third,0", state: "available" }]); await flush(); f.render();
  assert.equal(select(f.tree(), "-device").props.value, "hw:Third,0"); f.h.dispose();
});

test("an indeterminate check does not block Save and busy results never block edits to the current endpoint", async () => {
  for (const state of ["indeterminate", "current_sonante"]) {
    const f = await modal({ devices: availableDevices });
    choose(f, "hw:Other,0"); f.query.requests[0].resolve([{ id: "hw:Other,0", state }]); await flush(); f.render();
    await button(f.tree(), "settings.save").props.onClick();
    assert.equal(f.calls.save.length, 1, "the mandatory backend preflight remains authoritative"); f.h.dispose();
  }
  const f = await modal();
  button(f.tree(), "settings.deviceAvailability.check").props.onClick();
  f.query.requests[0].resolve([{ id: base.alsa_device, state: "busy" }]); await flush(); f.render();
  select(f.tree(), "-buffer").props.onChange({ target: { value: "32768" } }); f.render();
  await button(f.tree(), "settings.save").props.onClick();
  assert.equal(f.calls.availability, 1); assert.equal(f.calls.save.length, 1); f.h.dispose();
});

test("diagram stages are keyboard-native buttons; descriptions and collapsed details follow the draft route", () => {
  const h = component("AudioSignalDiagram"); const shared = { mode: "shared", device: "hw:Fixture,0" };
  let tree = h.render(shared);
  assert.ok(text(find(tree, node => node.props.role === "status")).includes("settings.signal.inspect"));
  const stage = button(tree, "settings.signal.system");
  assert.equal(stage.props.type, "button"); assert.equal(stage.props.tabIndex, undefined);
  stage.props.onClick(); tree = h.render(shared);
  assert.equal(button(tree, "settings.signal.system").props["aria-pressed"], true);
  assert.equal(button(tree, "settings.signal.system").props["aria-describedby"], find(tree, node => node.props.role === "status").props.id);
  assert.ok(text(find(tree, node => node.props.role === "status")).includes("settings.signal.systemDescription"));
  tree = h.render({ ...shared, mode: "direct" });
  assert.ok(text(find(tree, node => node.props.role === "status")).includes("settings.signal.inspect"));
  const alsa = find(tree, node => node.type === "button" && text(node).includes("settings.signal.alsa"));
  alsa.props.onClick(); tree = h.render({ ...shared, mode: "direct" });
  assert.ok(text(find(tree, node => node.props.role === "status")).includes("settings.signal.alsaDirect"));
  tree = h.render(shared);
  assert.ok(text(find(tree, node => node.props.role === "status")).includes("settings.signal.alsaShared"));
  const details = find(tree, node => node.type === "details");
  assert.equal(details.props.open, undefined); assert.equal(nodes(details).filter(node => node.type === "summary").length, 1);
  assert.ok(text(details).includes("settings.signal.notice")); h.dispose();
});

test("long processing/buffer explanations are collapsed while controls and urgent incompatibilities remain visible", async () => {
  const f = await modal();
  const audio = pageProps(f.tree()).audio;
  assert.equal(nodes(audio).filter(node => node.type === "details").length, 2);
  for (const details of nodes(audio).filter(node => node.type === "details")) assert.equal(details.props.open, undefined);
  assert.ok(nodes(audio).filter(node => node.type === "details").some(node => text(node).includes("settings.processingNotice")));
  assert.ok(nodes(audio).filter(node => node.type === "details").some(node => text(node).includes("settings.bufferNotice")));
  find(f.tree(), node => node.type === "button" && text(node).includes("settings.pipewireMode")).props.onClick(); f.render();
  assert.ok(text(pageProps(f.tree()).audio).includes("settings.dopOnlyDirect"));
  assert.equal(find(f.tree(), node => node.type === "input" && node.props["aria-label"] === "settings.dop").props.disabled, true);
  assert.ok(nodes(f.tree()).some(node => node.props.role === "status" && text(node).includes("settings.audioChangeNotice")));
  f.h.dispose();
});


test("first-run configuration is not presented as a PCM already owned by Sonante", async () => {
  const f = await modal({ config: { ...base, first_run: true } });
  assert.equal(f.calls.availability, 0);
  assert.ok(statusText(f).includes("settings.deviceAvailability.indeterminate"));
  assert.ok(!statusText(f).includes("settings.deviceAvailability.current_sonante"));
  f.h.dispose();
});

test("Sonante names the first interactive diagram stage in both languages", () => {
  for (const language of ["en", "pt-BR"]) {
    const resources = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8"));
    const translate = key => key.split(".").reduce((value, part) => value?.[part], resources);
    const h = component("AudioSignalDiagram", {
      "react-i18next": { useTranslation: () => ({ t: translate }) },
    });
    for (const mode of ["shared", "direct"]) {
      const props = { mode, device: base.alsa_device };
      let tree = h.render(props);
      const stage = button(tree, "Sonante");
      assert.equal(nodes(stage).find(node => node.type === icons.Music2).props.size, 24);
      stage.props.onClick(); tree = h.render(props);
      assert.equal(button(tree, "Sonante").props["aria-pressed"], true);
      assert.equal(text(find(tree, node => node.props.role === "status")), resources.settings.signal.sourceDescription);
      assert.match(resources.settings.signal.sourceDescription, /Sonante.*Local.*Plex/);
    }
    h.dispose();
  }
});

const modeButton = (tree, mode) => find(tree, node => node.type === "button" && text(node).includes(`settings.${mode}Mode`));

test("only an explicit Shared to Direct transition checks the previously selected enumerated DAC once", async () => {
  const f = await modal({ config: { ...base, audio_output_type: "pipewire" }, devices: availableDevices });
  assert.equal(f.calls.availability, 0, "opening Preferences never queries availability");
  modeButton(f.tree(), "pipewire").props.onClick(); f.render();
  assert.equal(f.calls.availability, 0);
  modeButton(f.tree(), "alsa").props.onClick(); f.render();
  assert.deepEqual(f.query.requests.map(request => request.id), [base.alsa_device]);
  modeButton(f.tree(), "alsa").props.onClick(); f.render();
  assert.equal(f.calls.availability, 1, "clicking the already selected mode is not another transition");
  f.query.requests[0].resolve([{ id: base.alsa_device, state: "available" }]); await flush(); f.render();
  assert.ok(statusText(f).includes("settings.deviceAvailability.available"));
  f.h.dispose();
});

test("returning to Direct never contextually queries Sonante's current DAC or unenumerated/virtual endpoints", async () => {
  for (const config of [base, { ...base, audio_output_type: "pipewire", alsa_device: "null" },
    { ...base, audio_output_type: "pipewire", alsa_device: "hw:NotEnumerated,0" }]) {
    const f = await modal({ config, devices: availableDevices });
    modeButton(f.tree(), "pipewire").props.onClick(); f.render();
    modeButton(f.tree(), "alsa").props.onClick(); f.render();
    select(f.tree(), "-device").props.onFocus(); f.render();
    assert.equal(f.calls.availability, 0); f.h.dispose();
  }
});

test("a fresh selector focus refreshes a busy observation once for mouse or keyboard, keeping manual retry", async () => {
  for (const input of ["mouse", "keyboard"]) {
    const f = await modal({ devices: availableDevices });
    choose(f, "hw:Other,0");
    f.query.requests[0].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
    const device = select(f.tree(), "-device");
    assert.equal(device.props.onMouseDown, undefined);
    assert.equal(device.props.onKeyDown, undefined, "no fragile popup-open gesture combination");
    device.props.onFocus({ input });
    device.props.onFocus({ input }); // Even repeated delivery before a render shares the pending IPC.
    f.render();
    assert.equal(f.calls.availability, 2);
    assert.ok(statusText(f).includes("settings.deviceAvailability.checking"));
    assert.equal(button(f.tree(), "settings.deviceAvailability.check").props.disabled, true);
    f.query.requests[1].resolve([{ id: "hw:Other,0", state: "available" }]); await flush(); f.render();
    assert.ok(statusText(f).includes("settings.deviceAvailability.available"));
    assert.ok(!nodes(f.tree()).some(node => node.props.role === "alert" && text(node).includes("settings.deviceAvailability.busyAction")));
    select(f.tree(), "-device").props.onFocus(); f.render();
    assert.equal(f.calls.availability, 2, "focus with an available/unknown/current status does not query");
    button(f.tree(), "settings.deviceAvailability.check").props.onClick(); f.render();
    assert.equal(f.calls.availability, 3); f.h.dispose();
    f.query.requests[2].resolve([{ id: "hw:Other,0", state: "available" }]); await flush();
  }
});

test("mode/focus contextual requests remain serialized, discard late results, and cannot interfere with Save", async () => {
  const f = await modal({ config: { ...base, audio_output_type: "pipewire" }, devices: availableDevices });
  modeButton(f.tree(), "alsa").props.onClick(); f.render();
  modeButton(f.tree(), "pipewire").props.onClick(); f.render();
  modeButton(f.tree(), "alsa").props.onClick(); f.render();
  choose(f, "hw:Other,0");
  assert.equal(f.calls.availability, 1);
  f.query.requests[0].resolve([{ id: base.alsa_device, state: "busy" }]); await flush(); f.render();
  assert.deepEqual(f.query.requests.map(request => request.id), [base.alsa_device, "hw:Other,0"]);
  f.query.requests[1].resolve([{ id: "hw:Other,0", state: "busy" }]); await flush(); f.render();
  const save = button(f.tree(), "settings.save").props.onClick(); f.render();
  select(f.tree(), "-device").props.onFocus();
  modeButton(f.tree(), "alsa").props.onClick(); f.render();
  assert.equal(f.calls.availability, 3, "only the existing Save revalidation is in flight");
  f.query.requests[2].resolve([{ id: "hw:Other,0", state: "available" }]); await save; await flush(); f.render();
  assert.equal(f.calls.save.length, 1); f.h.dispose();
});

test("compact availability uses short status text and a decorative icon without duplicating status in the selector", async () => {
  for (const [state, displayed] of [["available", "available"], ["busy", "busy"],
    ["indeterminate", "indeterminate"], ["current_sonante", "indeterminate"]]) {
    const f = await modal({ devices: availableDevices });
    choose(f, "hw:Other,0");
    let status = find(f.tree(), node => node.props.role === "status" && text(node).includes("settings.deviceAvailability.checking"));
    assert.equal(status.props["aria-live"], "polite");
    assert.equal(nodes(status).find(node => node.type === icons.LoaderCircle).props["aria-hidden"], "true");
    f.query.requests[0].resolve([{ id: "hw:Other,0", state }]); await flush(); f.render();
    status = find(f.tree(), node => node.props.role === "status" && text(node) === `settings.deviceAvailability.${displayed}`);
    assert.equal(nodes(status).filter(node => node.props["aria-hidden"] === "true").length, 1);
    const optionText = nodes(select(f.tree(), "-device")).filter(node => node.type === "option").map(text);
    assert.deepEqual(optionText, availableDevices.map(device => device.name));
    assert.equal(select(f.tree(), "-device").props.value, "hw:Other,0", "selecting the draft does not apply it");
    assert.equal(f.calls.save.length, 0); f.h.dispose();
  }
});

test("help is collapsed, keyboard-native and labelled; toggling it never queries or saves", async () => {
  const f = await modal();
  const help = () => find(f.tree(), node => node.type === "button" && node.props["aria-label"] === "settings.deviceAvailability.helpLabel");
  const panel = () => find(f.tree(), node => node.props.id === help().props["aria-controls"]);
  assert.equal(help().props.type, "button");
  assert.equal(help().props.tabIndex, undefined, "native button participates in keyboard focus");
  assert.equal(help().props.title, help().props["aria-label"]);
  assert.equal(help().props["aria-expanded"], false); assert.equal(panel().props.hidden, true);
  assert.equal(nodes(help()).find(node => node.type === icons.CircleHelp).props["aria-hidden"], "true");
  for (const key of ["helpObservation", "helpShared", "helpBusy", "helpLimits"]) {
    assert.ok(text(panel()).includes(`settings.deviceAvailability.${key}`));
  }
  help().props.onClick(); f.render();
  assert.equal(help().props["aria-expanded"], true); assert.equal(panel().props.hidden, false);
  help().props.onClick(); f.render();
  assert.equal(help().props["aria-expanded"], false); assert.equal(panel().props.hidden, true);
  assert.equal(f.calls.availability, 0); assert.equal(f.calls.save.length, 0); f.h.dispose();
});

test("inconclusive reads and configured ownership cannot claim that MPD is using the PCM", async () => {
  const f = await modal({ devices: availableDevices });
  assert.ok(statusText(f).includes("settings.deviceAvailability.indeterminate"));
  assert.ok(!statusText(f).includes("settings.deviceAvailability.current_sonante"));
  assert.equal(f.calls.availability, 0);
  choose(f, "hw:Other,0");
  f.query.requests[0].reject(new Error("private raw ALSA diagnostic")); await flush(); f.render();
  assert.ok(statusText(f).includes("settings.deviceAvailability.indeterminate"));
  assert.ok(!text(f.tree()).includes("private raw ALSA diagnostic"));
  const pages = component("SettingsPages");
  for (const id of ["local", "plex", "audio"]) {
    button(pages.render(pageProps(f.tree())), `settings.pages.${id}`).props.onClick(); f.render();
  }
  assert.equal(f.calls.availability, 1); pages.dispose(); f.h.dispose();
});

test("busy conflict stays visible with collapsed help and Save refuses it only after a fresh retry", async () => {
  const f = await modal({ config: { ...base, audio_output_type: "pipewire" }, devices: availableDevices });
  modeButton(f.tree(), "alsa").props.onClick(); f.render();
  f.query.requests[0].resolve([{ id: base.alsa_device, state: "busy" }]); await flush(); f.render();
  const conflict = () => find(f.tree(), node => node.props.role === "alert" && text(node) === "settings.deviceAvailability.busyAction");
  assert.equal(conflict().props.hidden, undefined);
  assert.equal(select(f.tree(), "-device").props.value, base.alsa_device);
  const help = find(f.tree(), node => node.type === "button" && node.props["aria-label"] === "settings.deviceAvailability.helpLabel");
  assert.equal(find(f.tree(), node => node.props.id === help.props["aria-controls"]).props.hidden, true);
  button(f.tree(), "settings.deviceAvailability.check").props.onClick(); f.render();
  assert.equal(f.calls.availability, 2);
  f.query.requests[1].resolve([{ id: base.alsa_device, state: "busy" }]); await flush(); f.render();
  const save = button(f.tree(), "settings.save").props.onClick(); f.render();
  assert.equal(f.calls.save.length, 0);
  f.query.requests[2].resolve([{ id: base.alsa_device, state: "busy" }]); await save; await flush(); f.render();
  conflict(); assert.equal(f.calls.save.length, 0); assert.equal(f.calls.started.length, 0);
  modeButton(f.tree(), "pipewire").props.onClick(); f.render();
  select(f.tree(), "-buffer").props.onChange({ target: { value: "32768" } }); f.render();
  await button(f.tree(), "settings.save").props.onClick();
  assert.equal(f.calls.availability, 3); assert.equal(f.calls.save.length, 1);
  assert.equal(f.calls.save[0].audio_output_type, "pipewire"); f.h.dispose();
});

test("overlap refusal in Settings retains the proposal and shows translated feedback", async () => {
  const alerts = [], oldAlert = globalThis.alert, oldError = console.error;
  globalThis.alert = message => alerts.push(message); console.error = () => {};
  let f;
  try {
    const draft = { ...base, first_run: true, local_folders: ["/music", "/music/reggae"] };
    f = await modal({ config: { ...base, first_run: true },
      props: { initialPage: "local", initialDraft: draft, initialFolderOverlap: true },
      save: async () => { throw "LOCAL_SOURCE_NEW_OVERLAP"; } });
    assert.ok(nodes(f.tree()).some(node => node.props.role === "alert" && text(node).includes("settings.localFolderOverlap")));
    select(f.tree(), "-buffer").props.onChange({ target: { value: "32768" } }); f.render();
    await button(f.tree(), "settings.save").props.onClick(); f.render();
    assert.equal(f.calls.config, 1, "no confirmed-config reload discards this refused draft");
    assert.equal(f.calls.closed, 0); assert.equal(f.calls.saved.length, 0);
    assert.equal(select(f.tree(), "-buffer").props.value, 32768);
    assert.deepEqual(f.calls.save[0].local_folders, draft.local_folders);
    assert.deepEqual(alerts, ["settings.localFolderOverlap"]);
  } finally { f?.h.dispose(); globalThis.alert = oldAlert; console.error = oldError; }
});

test("initial page survives config loading, tab selection survives rerenders, and all panels remain mounted", async () => {
  for (const initialPage of [undefined, "general", "local", "plex"]) {
    const f = await modal({ props: { initialPage } });
    assert.equal(pageProps(f.tree()).initialPage, initialPage, "forwarded after async config load");
    const pages = component("SettingsPages");
    let tree = pages.render(pageProps(f.tree()));
    const check = expected => {
      const tabs = nodes(tree).filter(node => node.props.role === "tab");
      const panels = nodes(tree).filter(node => node.props.role === "tabpanel");
      assert.equal(tabs.length, 4); assert.equal(panels.length, 4);
      for (let i = 0; i < tabs.length; i++) {
        const active = tabs[i].props.id.endsWith(`-tab-${expected}`);
        assert.equal(tabs[i].props["aria-selected"], active);
        assert.equal(tabs[i].props.tabIndex, active ? 0 : -1);
        assert.equal(panels[i].props.hidden, !active);
        assert.equal(tabs[i].props["aria-controls"], panels[i].props.id);
        assert.equal(panels[i].props["aria-labelledby"], tabs[i].props.id);
      }
    };
    check(initialPage ?? "general");
    const retained = nodes(tree).filter(node => node.props.role === "tabpanel").map(node => node.props.children[1]);
    button(tree, "settings.pages.audio").props.onClick();
    tree = pages.render(pageProps(f.tree())); check("audio");
    assert.deepEqual(nodes(tree).filter(node => node.props.role === "tabpanel").map(node => node.props.children[1]), retained);
    select(f.tree(), "-buffer").props.onChange({ target: { value: "32768" } });
    f.render(); tree = pages.render(pageProps(f.tree())); check("audio");
    tree = pages.render({ ...pageProps(f.tree()), initialPage: "plex" }); check("audio");
    assert.equal(select(f.tree(), "-buffer").props.value, 32768);
    pages.dispose(); f.h.dispose();
  }
});
