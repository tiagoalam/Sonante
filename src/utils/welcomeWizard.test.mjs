import * as frontendDiagnostics from "./frontendDiagnostics.ts";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDeviceAvailabilityQuery, shouldCheckContextualDevice } from "./deviceAvailability.ts";
import { wizardPlexConnectionReady, wizardSaveErrorKey } from "./welcomeWizard.ts";
import { createAppearance, APPEARANCE_KEY } from "./appearance.ts";

const source = readFileSync(new URL("../components/WelcomeWizard.tsx", import.meta.url), "utf8");
const flush = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const t = (key, vars) => key === "wizard.step" ? `${key}:${vars.step}/${vars.total}` : key;
const base = { first_run: true, audio_output_type: "pipewire", alsa_device: "default", local_folders: [],
  online_artwork_enabled: false, plex_url: "", plex_token: "", plex_server_id: null, plex_server_name: null,
  playback_mode: "http", local_mount_path: "", remote_share_path: "", dop_enabled: false, audio_buffer_size_kb: 8192, replay_gain: "off" };
const devices = ["hw:CARD=A,DEV=0", "hw:CARD=B,DEV=0", "hw:CARD=C,DEV=0"].map((id, i) => ({ id, name: `Fixture DAC ${i + 1}` }));
const server = (name = "Living room", id = "fixture-server") => ({ name, client_identifier: id,
  chosen_uri: "http://192.0.2.12:32400", connections: [{ uri: "http://192.0.2.12:32400", local: true, address: "192.0.2.12", port: 32400 }] });
const connected = { ...base, plex_token: "fixture-token", plex_server_id: "fixture-server", plex_server_name: "Living room", plex_url: server().chosen_uri };
function nodes(tree) {
  if (!React.isValidElement(tree)) return Array.isArray(tree) ? tree.flatMap(nodes) : [];
  return [tree, ...nodes(tree.props.children)];
}
const text = tree => React.isValidElement(tree) ? text(tree.props.children) : Array.isArray(tree) ? tree.map(text).join("") : typeof tree === "string" || typeof tree === "number" ? String(tree) : "";
const find = (tree, predicate) => { const value = nodes(tree).find(predicate); assert.ok(value, "Expected UI control exists"); return value; };
const button = (f, label) => find(f.tree, node => node.type === "button" && text(node) === label);
const mode = (f, id) => find(f.tree, node => node.type === "button" && text(node).includes(`settings.${id}Mode`));
const selector = (f, suffix) => find(f.tree, node => node.type === "select" && node.props.id.endsWith(suffix));
const sourceButton = (f, id) => find(f.tree, node => node.type === "button" && text(node).includes(`sidebar.${id}`));
const diagram = f => find(f.tree, node => node.type?.name === "AudioSignalDiagram").props;
const fixtures = [];
test.afterEach(() => { fixtures.splice(0).reverse().forEach(f => f.dispose()); });
function wizard(overrides = {}) {
  const slots = [], effects = [], timers = new Map();
  let cursor = 0, sequence = 0, timerSequence = 0;
  const calls = { reads: [], saves: [], finished: [], pins: 0, pinChecks: 0, servers: 0, browser: [], pickers: 0, focused: [], palettes: [], appearanceWrites: 0 };
  let appearanceRaw = overrides.appearanceRaw ?? null;
  const appearance = createAppearance({origin:"wizard-fixture",storage:()=>({getItem:()=>appearanceRaw,setItem:(key,value)=>{assert.equal(key,APPEARANCE_KEY);appearanceRaw=value;calls.appearanceWrites++;}}),apply:value=>calls.palettes.push(value),onStorage:()=>()=>{}});
  const stopAppearance = appearance.start();
  const appearanceCode = ts.transpileModule(readFileSync(new URL("../components/AppearanceSetting.tsx",import.meta.url),"utf8"),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
  const appearanceExports = {};
  const appearanceDeps = {react:{...React,useId:()=>"wizard-appearance-fixture",useSyncExternalStore:(_subscribe,get)=>get()},"react/jsx-runtime":jsx,"react-i18next":{useTranslation:()=>({t})},"../services/appearance":{appearance}};
  new Function("require","exports",appearanceCode)(id=>{assert.ok(id in appearanceDeps,id);return appearanceDeps[id];},appearanceExports);
  const previousWindow = globalThis.window, previousClear = globalThis.clearInterval;
  globalThis.window = { setInterval(callback, delay) { assert.equal(delay, 1500); timers.set(++timerSequence, callback); return timerSequence; } };
  globalThis.clearInterval = id => timers.delete(id);
  const hooks = { ...React,
    useState(initial) { const key = cursor++; if (!(key in slots)) slots[key] = typeof initial === "function" ? initial() : initial; return [slots[key], value => { slots[key] = typeof value === "function" ? value(slots[key]) : value; }]; },
    useRef(initial) { const key = cursor++; return slots[key] ??= { current: initial }; },
    useId() { const key = cursor++; return slots[key] ??= `wizard-fixture-${sequence++}`; },
    useEffect(effect, deps) { const key = cursor++, previous = slots[key]; if (!previous || deps.some((value, i) => !Object.is(value, previous.deps[i]))) effects.push(() => { previous?.cleanup?.(); slots[key] = { deps, cleanup: effect() }; }); },
  };
  const dependencies = {
    react: { ...hooks, default: hooks, __esModule: true }, "react/jsx-runtime": jsx,
    "../utils/frontendDiagnostics": frontendDiagnostics, "lucide-react": icons,
    "react-i18next": { useTranslation: () => ({ t, i18n: { language: "en", changeLanguage() {} } }) },
    "./AudioSignalDiagram": { AudioSignalDiagram: function AudioSignalDiagram() {} },
    "./AppearanceSetting": appearanceExports,
    "../utils/deviceAvailability": { createDeviceAvailabilityQuery, shouldCheckContextualDevice },
    "../utils/welcomeWizard": { wizardPlexConnectionReady, wizardSaveErrorKey },
    "../services/audio": { audioService: { async pickDirectory() { calls.pickers++; return "/fixture/local"; } } },
    "../services/config": { configService: {
      async checkDirectDeviceAvailability(id) { const d = deferred(); calls.reads.push({ id, ...d }); return d.promise; },
      async saveConfig(config) { calls.saves.push(config); return overrides.save ? overrides.save(config) : { status: "completed" }; },
    } },
    "../services/plex": { plexService: {
      async createPin() { calls.pins++; return overrides.createPin ? overrides.createPin() : { id: 7, code: "PIN7", auth_url: "https://example.invalid/auth" }; },
      async openExternalUrl(url) { calls.browser.push(url); if (overrides.openBrowser) return overrides.openBrowser(); },
      async checkPin() { calls.pinChecks++; return overrides.checkPin ? overrides.checkPin() : "fixture-token"; },
      async getServers() { calls.servers++; return overrides.getServers ? overrides.getServers() : overrides.servers ?? [server()]; },
    } },
  };
  const js = ts.transpileModule(source, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
  const exports = {};
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, `Unexpected import ${id}`); return dependencies[id]; }, exports);
  let disposed = false;
  const f = { calls, timers, appearance, appearanceRaw:()=>appearanceRaw, tree: null,
    render() { cursor = 0; f.tree = exports.WelcomeWizard({ initialConfig: overrides.config ?? base, devices: overrides.devices ?? devices, onCancel: overrides.onCancel, onSaveAccepted: overrides.onSaveAccepted, onFinish(config) { calls.finished.push(config); } }); for(const node of nodes(f.tree))if(node.type==="h2"&&node.props.ref){node.props.ref.current={focus(){calls.focused.push(text(node));}};} effects.splice(0).forEach(run => run()); return f.tree; },
    async tick() { const callback = [...timers.values()][0]; assert.ok(callback, "existing PIN timer is active"); await callback(); await flush(); f.render(); },
    dispose() { if (disposed) return; disposed = true; stopAppearance(); slots.forEach(slot => slot?.cleanup?.()); globalThis.window = previousWindow; globalThis.clearInterval = previousClear; },
  };
  fixtures.push(f); f.render(); return f;
}
const next = f => { const b = button(f, "wizard.next"); assert.equal(Boolean(b.props.disabled), false); b.props.onClick(); f.render(); };
const back = f => { button(f, "wizard.back").props.onClick(); f.render(); };
const choose = (f, id) => { selector(f, "-device").props.onChange({ target: { value: id } }); f.render(); };
const selectSources = (f, local, plex) => {
  next(f);
  if (!local) { sourceButton(f, "local").props.onClick(); f.render(); }
  if (plex) { sourceButton(f, "plex").props.onClick(); f.render(); }
  next(f);
};
const enterPlex = (f, local = false) => { selectSources(f, local, true); if (local) next(f); };
const auth = async f => { await button(f, "settings.connectPlex").props.onClick(); f.render(); await f.tick(); };
const statusText = f => nodes(f.tree).filter(node => node.props.role === "status").map(text).join(" ");

const audioWizard = (overrides={}) => {const f=wizard(overrides);if(!overrides.onCancel)next(f);return f;};

test("opening preserves saved/default/missing DACs without probes, enumeration or silent replacement", () => {
  for (const id of ["default", "", "hw:CARD=Missing,DEV=0", devices[0].id]) {
    const f = audioWizard({ config: { ...base, audio_output_type: "alsa", alsa_device: id } });
    assert.equal(selector(f, "-device").props.value, id); assert.equal(f.calls.reads.length, 0); assert.equal(f.calls.servers, 0);
    const values = nodes(selector(f, "-device")).filter(node => node.type === "option").map(node => node.props.value);
    assert.ok(values.includes(id)); assert.match(statusText(f), /indeterminate/); f.dispose();
  }
  assert.doesNotMatch(source, /devices\[0\]|getAudioDevices\(/);
});

test("explicit Shared/Direct and new physical choices update the diagram and serialize only directed checks", async () => {
  const f = audioWizard({ config: { ...base, alsa_device: devices[0].id } });
  assert.equal(diagram(f).mode, "shared");
  mode(f, "alsa").props.onClick(); f.render();
  assert.equal(mode(f, "alsa").props["aria-pressed"], true); assert.equal(diagram(f).mode, "direct");
  assert.deepEqual(f.calls.reads.map(row => row.id), [devices[0].id]);
  mode(f, "alsa").props.onClick(); f.render(); assert.equal(f.calls.reads.length, 1);
  choose(f, devices[1].id); choose(f, devices[2].id);
  assert.equal(diagram(f).device, devices[2].id); assert.equal(f.calls.reads.length, 1);
  f.calls.reads[0].resolve([{ id: devices[0].id, state: "busy" }]); await flush(); f.render();
  assert.deepEqual(f.calls.reads.map(row => row.id), [devices[0].id, devices[2].id]); assert.doesNotMatch(statusText(f), /\.busy/);
  f.calls.reads[1].resolve([{ id: devices[2].id, state: "available" }]); await flush(); f.render();
  assert.match(statusText(f), /\.available/);
  mode(f, "pipewire").props.onClick(); f.render(); assert.equal(diagram(f).mode, "shared");
});

test("available, busy, inconclusive and ownership-marker results use the same compact presentation as Settings", async () => {
  for (const state of ["available", "busy", "indeterminate", "current_sonante"]) {
    const f = audioWizard(); mode(f, "alsa").props.onClick(); f.render(); choose(f, devices[0].id);
    assert.match(statusText(f), /\.checking/);
    f.calls.reads[0].resolve([{ id: devices[0].id, state }]); await flush(); f.render();
    assert.match(statusText(f), new RegExp(`\\.${state === "current_sonante" ? "indeterminate" : state}`));
    assert.doesNotMatch(statusText(f), /current_sonante/); assert.equal(selector(f, "-device").props.value, devices[0].id); f.dispose();
  }
});

test("help and retry are keyboard-native; a busy observation can become available without a permanent Save lock", async () => {
  const f = audioWizard(); mode(f, "alsa").props.onClick(); f.render(); choose(f, devices[0].id);
  f.calls.reads[0].resolve([{ id: devices[0].id, state: "busy" }]); await flush(); f.render();
  const help = () => find(f.tree, node => node.type === "button" && node.props["aria-label"] === "settings.deviceAvailability.helpLabel");
  const panel = () => find(f.tree, node => node.props.id === help().props["aria-controls"]);
  assert.equal(help().props.type, "button"); assert.equal(help().props["aria-expanded"], false); assert.equal(panel().props.hidden, true);
  help().props.onClick(); f.render(); assert.equal(panel().props.hidden, false);
  assert.match(text(panel()), /helpObservation.*helpShared.*helpBusy.*helpLimits/); assert.equal(f.calls.reads.length, 1);
  button(f, "settings.deviceAvailability.check").props.onClick(); button(f, "settings.deviceAvailability.check").props.onClick(); f.render();
  assert.equal(f.calls.reads.length, 2);
  f.calls.reads[1].resolve([{ id: devices[0].id, state: "available" }]); await flush(); f.render();
  assert.match(statusText(f), /\.available/);
});

test("current Direct owner, virtual/unlisted choices and page navigation do not trigger extra probes", () => {
  const f = audioWizard({ config: { ...base, first_run: false, audio_output_type: "alsa", alsa_device: devices[0].id } });
  mode(f, "pipewire").props.onClick(); f.render(); mode(f, "alsa").props.onClick(); f.render();
  assert.equal(f.calls.reads.length, 0); assert.equal(button(f, "settings.deviceAvailability.check").props.disabled, true);
  choose(f, "null"); choose(f, "hw:CARD=Unlisted,DEV=0"); next(f); back(f);
  assert.equal(f.calls.reads.length, 0); assert.equal(diagram(f).device, "hw:CARD=Unlisted,DEV=0");
});

test("source controls are semantic toggle buttons and all optional combinations reach review before Save", async () => {
  for (const [local, plex, total] of [[true, false, 5], [false, true, 5], [true, true, 6]]) {
    const f = audioWizard({ config: plex ? connected : base });
    next(f); const localToggle = sourceButton(f, "local"), plexToggle = sourceButton(f, "plex");
    assert.equal(localToggle.props.type, "button"); assert.equal(localToggle.props["aria-pressed"], true);
    assert.equal(plexToggle.props.type, "button"); assert.equal(plexToggle.props["aria-pressed"], false);
    assert.equal(nodes(f.tree).filter(node => node.type === "input" && node.props.type === "checkbox").length, 0);
    if (!local) { localToggle.props.onClick(); f.render(); }
    if (plex) { sourceButton(f, "plex").props.onClick(); f.render(); }
    assert.match(text(f.tree), new RegExp(`wizard.step:3/${total}`));
    next(f);
    if (local) { await button(f, "settings.addFolder").props.onClick(); f.render(); assert.ok(text(f.tree).includes("/fixture/local")); next(f); }
    if (plex) { assert.match(text(f.tree), /wizard.connected/); next(f); }
    assert.match(text(f.tree), /wizard.stepFinalTitle/); assert.match(text(f.tree), new RegExp(`wizard.step:${total}/${total}`));
    assert.equal(f.calls.saves.length, 0);
    await button(f, "wizard.start").props.onClick(); f.render();
    assert.equal(f.calls.saves.length, 1); assert.equal(f.calls.saves[0].first_run, false);
    assert.equal(f.calls.finished.length, 1); assert.deepEqual(f.calls.finished[0], f.calls.saves[0]); f.dispose();
  }
});

test("back/next retains the audio and folder drafts; layout scrolls within the viewport with a fixed footer", async () => {
  const f = audioWizard(); mode(f, "alsa").props.onClick(); f.render(); choose(f, devices[0].id);
  f.calls.reads[0].resolve([{ id: devices[0].id, state: "available" }]); await flush(); f.render();
  next(f); next(f); await button(f, "settings.addFolder").props.onClick(); f.render();
  back(f); back(f); assert.equal(selector(f, "-device").props.value, devices[0].id); assert.equal(diagram(f).mode, "direct");
  next(f); next(f); assert.ok(text(f.tree).includes("/fixture/local")); next(f);
  assert.ok(text(f.tree).includes("Fixture DAC 1")); assert.match(text(f.tree), /wizard.stepFinalTitle/);
  const dialog = find(f.tree, node => node.props.role === "dialog");
  assert.ok(dialog.props["aria-labelledby"]); assert.ok(dialog.props.className.includes("94dvh"));
  assert.equal(nodes(f.tree).filter(node => node.props.className?.includes("flex-1 min-h-0 overflow-y-auto")).length, 1);
  assert.ok(!nodes(find(f.tree, node => node.props.className?.includes("overflow-y-auto"))).includes(button(f, "wizard.start")));
});

test("PIN authentication retains backend route selection, hides all connection details and offers name-only server selection", async () => {
  const servers = [server(), { ...server("Study", "second-server"), chosen_uri: "https://example.invalid:32401" }];
  const f = audioWizard({ servers }); enterPlex(f); await auth(f);
  assert.equal(f.calls.pins, 1); assert.equal(f.calls.pinChecks, 1); assert.equal(f.calls.servers, 1); assert.equal(f.timers.size, 0);
  assert.match(statusText(f), /wizard.connected/); assert.equal(selector(f, "-server").props.value, "fixture-server");
  assert.deepEqual(nodes(selector(f, "-server")).filter(node => node.type === "option").map(text), ["Living room", "Study"]);
  const markup = renderToStaticMarkup(f.tree).replaceAll('xmlns="http://www.w3.org/2000/svg"', "");
  assert.doesNotMatch(markup, /192\.0\.2\.12|32400|32401|https?:|fixture-token/);
  selector(f, "-server").props.onChange({ target: { value: "second-server" } }); f.render(); next(f);
  assert.match(text(f.tree), /Study/);
  await button(f, "wizard.start").props.onClick();
  assert.equal(f.calls.finished[0].plex_server_id, "second-server"); assert.equal(f.calls.finished[0].plex_url, servers[1].chosen_uri);
});

test("one connected Plex server shows only its friendly name and disconnect clears the internal connection", async () => {
  const f = audioWizard(); enterPlex(f); await auth(f);
  assert.match(text(f.tree), /Living room/); assert.ok(!nodes(f.tree).some(node => node.type === "select"));
  button(f, "settings.disconnect").props.onClick(); f.render();
  assert.doesNotMatch(statusText(f), /wizard.connected/); assert.ok(button(f, "settings.connectPlex"));
  assert.equal(button(f, "wizard.next").props.disabled, true);
});

test("authentication with no server, invalid route or missing identity never announces a complete connection", async () => {
  for (const servers of [[], [{ ...server(), chosen_uri: "invalid" }], [{ ...server(), client_identifier: "" }]]) {
    const f = audioWizard({ servers }); enterPlex(f); await auth(f);
    assert.doesNotMatch(statusText(f), /wizard.connected/); assert.equal(button(f, "wizard.next").props.disabled, true);
    assert.equal(f.calls.saves.length, 0); assert.equal(f.calls.finished.length, 0);
    if (!servers.length) assert.match(text(f.tree), /wizard.noPlexServers/); f.dispose();
  }
  for (const config of [{ ...connected, plex_url: "" }, { ...connected, plex_server_id: null }, { ...connected, plex_token: "" }]) {
    const f = audioWizard({ config }); enterPlex(f); assert.doesNotMatch(statusText(f), /wizard.connected/); f.dispose();
  }
});

test("Plex discovery failure is translated and cannot expose raw credentials or announce success", async () => {
  const f = audioWizard({ getServers() { throw new Error("https://user:SECRET@192.0.2.1/?X-Plex-Token=SECRET"); } });
  enterPlex(f); await auth(f);
  assert.match(text(f.tree), /wizard.plexDiscoveryError/); assert.doesNotMatch(text(f.tree), /SECRET|https?:/);
  assert.doesNotMatch(statusText(f), /wizard.connected/); assert.equal(button(f, "wizard.next").props.disabled, true); assert.equal(f.timers.size, 0);
});

test("PIN cancel and unmount discard pending authentication/availability results and clean existing timers", async () => {
  const pin = deferred(), f = audioWizard({ createPin: () => pin.promise }); enterPlex(f);
  const start = button(f, "settings.connectPlex").props.onClick(); f.render();
  button(f, "settings.cancel").props.onClick(); f.render(); pin.resolve({ id: 8, code: "OLD", auth_url: "https://example.invalid" });
  await start; await flush(); f.render(); assert.equal(f.calls.browser.length, 0); assert.equal(f.timers.size, 0);
  const f2 = audioWizard({ checkPin: () => pin.promise }); enterPlex(f2);
  await button(f2, "settings.connectPlex").props.onClick(); f2.render(); assert.equal(f2.timers.size, 1); f2.dispose(); assert.equal(f2.timers.size, 0);
  const f3 = audioWizard(); mode(f3, "alsa").props.onClick(); f3.render(); choose(f3, devices[0].id); choose(f3, devices[1].id);
  f3.dispose(); f3.calls.reads[0].resolve([{ id: devices[0].id, state: "busy" }]); await flush(); assert.equal(f3.calls.reads.length, 1);
});

test("late server discovery after disconnect cannot reconnect or replace a new authentication session", async () => {
  const old = deferred(); let reads = 0;
  const f = audioWizard({ getServers: () => ++reads === 1 ? old.promise : [server("New server", "new-server")] }); enterPlex(f);
  await button(f, "settings.connectPlex").props.onClick(); f.render();
  const pending = [...f.timers.values()][0](); await flush(); f.render();
  assert.match(statusText(f), /settings.searchingServers/); button(f, "settings.disconnect").props.onClick(); f.render();
  await auth(f); assert.match(text(f.tree), /New server/);
  old.resolve([server("Old server", "old-server")]); await pending; await flush(); f.render();
  assert.match(text(f.tree), /New server/); assert.doesNotMatch(text(f.tree), /Old server/);
});

test("failed/busy/uncertain/accepted Save never finishes early or discards the review draft; retries remain possible", async () => {
  for (const error of ["ALSA_PREFLIGHT_OUTPUT_BUSY", "ALSA_PREFLIGHT_OUTPUT_UNAVAILABLE", "CONFIG_PERSISTENCE_UNCERTAIN: private", "RAW SECRET", "LOCAL_SOURCE_NEW_OVERLAP", "FIRST_RUN_DATABASE_UNCONFIRMED"]) {
    let attempts = 0;
    const f = audioWizard({ save: async () => { if (++attempts === 1) throw error; return { status: "completed" }; } });
    selectSources(f, true, false); await button(f, "settings.addFolder").props.onClick(); f.render(); next(f);
    await button(f, "wizard.start").props.onClick(); f.render();
    assert.equal(f.calls.finished.length, 0); assert.match(text(f.tree), /wizard.stepFinalTitle/);
    assert.match(text(f.tree), new RegExp(wizardSaveErrorKey(error).replace('.', '\\.'))); assert.doesNotMatch(text(f.tree), /RAW SECRET|private/);
    back(f); assert.match(text(f.tree), /\/fixture\/local/); next(f);
    await button(f, "wizard.start").props.onClick(); assert.equal(f.calls.finished.length, 1); f.dispose();
  }
  const f = audioWizard({ save: async () => ({ status: "accepted", operation: { id: "op", phase: "preparing" } }) });
  selectSources(f, true, false); next(f); await button(f, "wizard.start").props.onClick(); f.render(); assert.equal(f.calls.finished.length, 0);
});

test("postcommit preparation failure preserves the draft and blocks another completion", async () => {
  const f = audioWizard({ save: async () => { throw "FIRST_RUN_PREPARATION_INCOMPLETE: private"; } });
  selectSources(f, true, false); await button(f, "settings.addFolder").props.onClick(); f.render(); next(f);
  const complete = button(f, "wizard.start").props.onClick;
  await complete(); f.render();
  assert.equal(f.calls.finished.length, 0); assert.equal(f.calls.saves.length, 1);
  assert.match(text(f.tree), /wizard.preparationIncomplete/); assert.doesNotMatch(text(f.tree), /private/);
  assert.equal(button(f, "wizard.start").props.disabled, true);
  assert.equal(button(f, "wizard.back").props.disabled, true);
  await complete(); await button(f, "wizard.start").props.onClick();
  assert.equal(f.calls.saves.length, 1); assert.equal(f.calls.finished.length, 0);
  assert.deepEqual(f.calls.saves[0].local_folders, ["/fixture/local"]); f.dispose();
});

test("pending Save and PIN reads are deduplicated; audio queries cannot run during conclusion", async () => {
  const save = deferred(); const f = audioWizard({ save: () => save.promise }); selectSources(f, true, false); next(f);
  const complete = button(f, "wizard.start").props.onClick(); button(f, "wizard.start").props.onClick(); f.render();
  assert.equal(f.calls.saves.length, 1); assert.equal(button(f, "wizard.starting").props.disabled, true); assert.equal(button(f, "wizard.back").props.disabled, true);
  assert.equal(f.calls.finished.length, 0); assert.equal(f.calls.reads.length, 0);
  save.resolve({ status: "completed" }); await complete; assert.equal(f.calls.finished.length, 1); f.dispose();
  const pin = deferred(); const f2 = audioWizard({ checkPin: () => pin.promise }); enterPlex(f2);
  const start = button(f2, "settings.connectPlex").props.onClick(); button(f2, "settings.connectPlex").props.onClick(); await start; f2.render();
  assert.equal(f2.calls.pins, 1); const callback = [...f2.timers.values()][0]; const read = callback(); await callback(); assert.equal(f2.calls.pinChecks, 1);
  pin.resolve(null); await read;
});

test("connection readiness and completion diagnostics are conservative, translated and contain no raw error data", () => {
  assert.equal(wizardPlexConnectionReady(connected), true);
  for (const route of ["", "file:///private", "http://user:SECRET@example.invalid", "https://example.invalid/?X-Plex-Token=SECRET", "invalid"]) {
    assert.equal(wizardPlexConnectionReady({ ...connected, plex_url: route }), false);
  }
  assert.equal(wizardSaveErrorKey(new Error("ALSA_PREFLIGHT_OUTPUT_BUSY")), "wizard.deviceBusy");
  assert.equal(wizardSaveErrorKey(new Error("CONFIG_PERSISTENCE_UNCERTAIN: SECRET")), "wizard.configPersistenceUncertain");
  assert.equal(wizardSaveErrorKey("FIRST_RUN_DATABASE_UNCONFIRMED. Estado anterior restaurado."), "wizard.libraryPreparationError");
  assert.equal(wizardSaveErrorKey("FIRST_RUN_PREPARATION_INCOMPLETE: SECRET"), "wizard.preparationIncomplete");
  for (const language of ["en", "pt-BR"]) {
    const locale = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8"));
    for (const key of ["libraryPreparationError", "preparationIncomplete", "deviceBusy", "deviceUnavailable", "folderError", "noPlexServers", "plexDiscoveryError", "plexNotReady", "unnamedServer"]) assert.ok(locale.wizard[key]);
    assert.match(locale.wizard.step, /{{step}}.*{{total}}/);
  }
});

test("a stale busy observation cannot veto completion; only the Save backend accepts or refuses it", async () => {
  const f = audioWizard(); mode(f, "alsa").props.onClick(); f.render(); choose(f, devices[0].id);
  f.calls.reads[0].resolve([{ id: devices[0].id, state: "busy" }]); await flush(); f.render();
  selectSources(f, true, false); next(f);
  assert.equal(button(f, "wizard.start").props.disabled, false);
  await button(f, "wizard.start").props.onClick();
  assert.equal(f.calls.reads.length, 1); assert.equal(f.calls.saves.length, 1); assert.equal(f.calls.finished.length, 1);
});

test("PIN creation, browser and polling failures are translated, cancelable and never publish raw secrets", async () => {
  for (const overrides of [{ createPin() { throw new Error("SECRET"); } }, { openBrowser() { throw new Error("SECRET"); } }]) {
    const f = audioWizard(overrides); enterPlex(f);
    await button(f, "settings.connectPlex").props.onClick(); f.render();
    assert.match(text(f.tree), /settings.plexConnectionError/); assert.doesNotMatch(text(f.tree), /SECRET/);
    assert.equal(f.timers.size, 0); assert.ok(button(f, "settings.connectPlex")); f.dispose();
  }
  const f = audioWizard({ checkPin() { throw new Error("SECRET"); } }); enterPlex(f);
  await button(f, "settings.connectPlex").props.onClick(); f.render(); await f.tick();
  assert.match(text(f.tree), /settings.plexConnectionError/); assert.doesNotMatch(text(f.tree), /SECRET/);
  assert.equal(f.calls.servers, 0); button(f, "settings.cancel").props.onClick(); f.render(); assert.equal(f.timers.size, 0);
});

test("retained configuration is reviewed truthfully and an incomplete saved Plex reference cannot be submitted", async () => {
  const f = audioWizard({ config: { ...connected, local_folders: ["/fixture/retained"] } });
  selectSources(f, true, false); next(f);
  assert.match(text(f.tree), /wizard.connected.*Living room/);
  await button(f, "wizard.start").props.onClick();
  assert.deepEqual(f.calls.finished[0].local_folders, ["/fixture/retained"]);
  assert.equal(f.calls.finished[0].plex_url, connected.plex_url); f.dispose();
  const incomplete = audioWizard({ config: { ...connected, plex_server_id: null } }); selectSources(incomplete, true, false); next(incomplete);
  assert.match(text(incomplete.tree), /wizard.plexNotReady/);
  assert.equal(button(incomplete, "wizard.start").props.disabled, true);
  await button(incomplete, "wizard.start").props.onClick(); assert.equal(incomplete.calls.saves.length, 0);
});


test("reopened collection wizard prefills existing sources, cancel/reopen is read-only and accepted saves use the existing monitor",async()=>{
  const existing={...connected,first_run:false,local_folders:["/fixture/existing"]};
  const before=JSON.stringify(existing);let cancelled=0;
  for(let i=0;i<2;i++){
    const f=audioWizard({config:existing,onCancel(){cancelled++;}});
    assert.equal(sourceButton(f,"local").props["aria-pressed"],true);assert.equal(sourceButton(f,"plex").props["aria-pressed"],true);
    next(f);assert.match(text(f.tree),/fixture\/existing/);
    await button(f,"settings.addFolder").props.onClick();f.render();assert.match(text(f.tree),/fixture\/local/);
    button(f,"settings.cancel").props.onClick();assert.equal(cancelled,i+1);assert.equal(f.calls.saves.length,0);assert.equal(f.calls.finished.length,0);assert.equal(f.calls.servers,0);assert.equal(f.calls.reads.length,0);
    f.dispose();assert.equal(JSON.stringify(existing),before);
  }
  const onlyPlex=audioWizard({config:{...connected,first_run:false,local_folders:[]},onCancel(){}});
  assert.equal(sourceButton(onlyPlex,"local").props["aria-pressed"],false);assert.equal(sourceButton(onlyPlex,"plex").props["aria-pressed"],true);
  next(onlyPlex);next(onlyPlex);await button(onlyPlex,"wizard.start").props.onClick();
  assert.equal(onlyPlex.calls.finished.length,1);assert.equal(onlyPlex.calls.finished[0].plex_token,connected.plex_token);assert.deepEqual(onlyPlex.calls.finished[0].local_folders,[]);assert.equal(onlyPlex.calls.finished[0].audio_output_type,connected.audio_output_type);onlyPlex.dispose();
  const operation={id:"fixture-operation",phase:"preparing"};let accepted;
  const f=audioWizard({config:{...base,first_run:false},onCancel(){},save:async()=>({status:"accepted",operation}),onSaveAccepted(value){accepted=value;}});
  next(f);await button(f,"settings.addFolder").props.onClick();f.render();next(f);
  await button(f,"wizard.start").props.onClick();assert.equal(accepted,operation);assert.equal(f.calls.finished.length,0);assert.equal(f.calls.saves[0].first_run,false);f.dispose();
});


const appearanceRadios = f => {
  const child=find(f.tree,node=>node.type?.name==="AppearanceSetting");
  return nodes(child.type(child.props)).filter(node=>node.type==="input"&&node.props.type==="radio");
};
const appearanceSelect = f => ({props:{
 value:appearanceRadios(f).find(node=>node.props.checked).props.value,
 onChange:event=>appearanceRadios(f).find(node=>node.props.value===event.target.value).props.onChange(),
}});

test("appearance is the first initial step; reconfiguration starts at Sources by identity, with correct counters and focus",()=>{
 const f=wizard();assert.match(text(f.tree),/wizard.appearanceTitle/);assert.match(text(f.tree),/wizard.step:1\/5/);
 assert.equal(f.calls.focused.at(-1),"wizard.appearanceTitle");assert.equal(nodes(f.tree).some(n=>n.type==="button"&&text(n)==="wizard.back"),false);
 next(f);assert.match(text(f.tree),/wizard.step1Title/);assert.match(text(f.tree),/wizard.step:2\/5/);assert.equal(f.calls.focused.at(-1),"wizard.step1Title");
 next(f);assert.match(text(f.tree),/wizard.step2Title/);assert.match(text(f.tree),/wizard.step:3\/5/);
 back(f);back(f);assert.match(text(f.tree),/wizard.appearanceTitle/);assert.equal(f.calls.focused.at(-1),"wizard.appearanceTitle");
 const reopened=wizard({config:{...connected,first_run:false,local_folders:["/fixture/retained"]},onCancel(){}});
 assert.match(text(reopened.tree),/wizard.step2Title/);assert.match(text(reopened.tree),/wizard.step:3\/6/);assert.equal(reopened.calls.focused.at(-1),"wizard.step2Title");
 assert.equal(sourceButton(reopened,"local").props["aria-pressed"],true);assert.equal(sourceButton(reopened,"plex").props["aria-pressed"],true);
 back(reopened);assert.match(text(reopened.tree),/wizard.step1Title/);back(reopened);assert.match(text(reopened.tree),/wizard.appearanceTitle/);
});

test("the real appearance control retains an existing choice and persists independently without completing or saving setup",()=>{
 const raw=JSON.stringify({version:1,value:"light",revision:{counter:4,origin:"saved"}}),config={...connected,local_folders:["/fixture/retained"]},before=JSON.stringify(config);
 const f=wizard({config,appearanceRaw:raw});assert.equal(appearanceSelect(f).props.value,"light");assert.equal(f.appearanceRaw(),raw);assert.equal(f.calls.appearanceWrites,0);
 appearanceSelect(f).props.onChange({target:{value:"dark"}});f.render();assert.equal(f.appearance.getSnapshot(),"dark");assert.equal(JSON.parse(f.appearanceRaw()).value,"dark");
 next(f);back(f);assert.equal(appearanceSelect(f).props.value,"dark");const saved=f.appearanceRaw();f.dispose();
 const reopened=wizard({config,appearanceRaw:saved});assert.equal(appearanceSelect(reopened).props.value,"dark");
 assert.equal(JSON.stringify(config),before);for(const sample of [f,reopened]){assert.equal(sample.calls.saves.length,0);assert.equal(sample.calls.finished.length,0);assert.equal(sample.calls.reads.length,0);assert.equal(sample.calls.pins,0);assert.equal(sample.calls.servers,0);assert.equal(sample.calls.pickers,0);}
 const invalid=wizard({appearanceRaw:'bad'});assert.equal(appearanceSelect(invalid).props.value,"dark");
});

test("Sources remains mandatory after the new opening page; Local/Plex combinations keep review final",()=>{
 const previous=globalThis.alert,alerts=[];globalThis.alert=message=>alerts.push(message);
 try{const f=wizard();next(f);next(f);sourceButton(f,"local").props.onClick();f.render();next(f);assert.match(text(f.tree),/wizard.step2Title/);assert.deepEqual(alerts,["wizard.sourceRequired"]);assert.equal(f.calls.saves.length,0);sourceButton(f,"plex").props.onClick();f.render();next(f);assert.match(text(f.tree),/wizard.step4PlexTitle/);assert.equal(button(f,"wizard.next").props.disabled,true);assert.equal(f.calls.pins,0);}finally{globalThis.alert=previous;}
 for(const language of ["en","pt-BR"]){const locale=JSON.parse(readFileSync(new URL(`../locales/${language}.json`,import.meta.url),"utf8"));assert.ok(locale.wizard.appearanceTitle);assert.ok(locale.settings.appearanceDark);assert.ok(locale.settings.appearanceLight);}
});

test("returning to appearance preserves audio/folder drafts and active Plex PIN until ordinary cancel",async()=>{
 const f=wizard();next(f);mode(f,"alsa").props.onClick();f.render();choose(f,devices[0].id);f.calls.reads[0].resolve([{id:devices[0].id,state:"available"}]);await flush();f.render();
 next(f);sourceButton(f,"plex").props.onClick();f.render();next(f);await button(f,"settings.addFolder").props.onClick();f.render();next(f);
 await button(f,"settings.connectPlex").props.onClick();f.render();assert.equal(f.timers.size,1);
 back(f);back(f);back(f);back(f);appearanceSelect(f).props.onChange({target:{value:"light"}});f.render();assert.equal(f.calls.pins,1);assert.equal(f.timers.size,1);
 next(f);assert.equal(selector(f,"-device").props.value,devices[0].id);next(f);next(f);assert.match(text(f.tree),/fixture\/local/);next(f);assert.match(text(f.tree),/PIN7/);assert.equal(f.calls.saves.length,0);assert.equal(f.calls.finished.length,0);button(f,"settings.cancel").props.onClick();f.render();assert.equal(f.timers.size,0);
});
