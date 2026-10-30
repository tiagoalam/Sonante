import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { createQueueClearConfirmationFlow, queueClearBinding } from "./queueClearConfirmation.ts";
import { monitorConfigSave } from "./configSaveMonitor.ts";
import { configService } from "../services/config.ts";

const binding = "a".repeat(64);
const nextBinding = "b".repeat(64);
const required = (value = binding) => `LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED:${value}`;
const restored = ". Estado anterior restaurado.";
const operation = (id, phase, error = null) => ({ id, phase, error });
const proposal = () => ({
  first_run: false, alsa_device: "default", audio_output_type: "pipewire",
  local_folders: ["/tmp/fixture-kept"], online_artwork_enabled: false,
  plex_url: "", plex_token: "", playback_mode: "http", local_mount_path: "",
  remote_share_path: "", dop_enabled: false, audio_buffer_size_kb: 2048, replay_gain: "off",
});
const previousWindow = globalThis.window;
afterEach(() => {
  if (previousWindow === undefined) delete globalThis.window;
  else globalThis.window = previousWindow;
});

function harness() {
  const flow = createQueueClearConfirmationFlow();
  const calls = [], changes = [], dialogs = [], timers = new Map();
  let id = 0, timerId = 0, resolveRead;
  globalThis.window = { __TAURI_INTERNALS__: { invoke: async (command, args) => {
    calls.push({ command, args });
    assert.equal(command, "save_config");
    return { status: "accepted", operation: operation(String(++id), "preparing") };
  } } };
  const monitor = monitorConfigSave(
    () => new Promise(resolve => { resolveRead = resolve; }),
    op => { changes.push(op); const dialog = flow.observe(op); if (dialog) dialogs.push(dialog); },
    assert.fail,
    callback => { timers.set(++timerId, callback); return timerId; },
    timer => timers.delete(timer),
  );
  const settleRead = async op => {
    resolveRead(op);
    await Promise.resolve(); await Promise.resolve();
  };
  return {
    flow, calls, changes, dialogs, monitor, settleRead, timers,
    async start(config = proposal(), authorization) {
      assert.equal(monitor.isPending(), false, "concurrent admission must be refused");
      const snapshot = flow.begin(config);
      monitor.beginAcceptance();
      const response = await configService.saveConfig(snapshot, authorization);
      flow.accepted(response.operation.id);
      monitor.accepted(response.operation);
      return response.operation.id;
    },
    async poll(op) {
      await settleRead(op);
      const [timer, callback] = timers.entries().next().value;
      timers.delete(timer); callback();
    },
    dispose() { flow.discard(); monitor.dispose(); },
  };
}

test("normal Save keeps the old IPC payload and never asks for queue confirmation", async () => {
  const f = harness();
  const id = await f.start();
  assert.deepEqual(f.calls[0], { command: "save_config", args: { newConfig: proposal() } });
  assert.ok(f.monitor.isPending());
  await f.poll(operation(id, "indexing"));
  assert.equal(f.dialogs.length, 0);
  await f.poll(operation(id, "completed"));
  assert.equal(f.dialogs.length, 0);
  assert.equal(f.monitor.isPending(), false);
  f.dispose();
});

test("real confirmed-rollback error opens confirmation through the Save monitor", async () => {
  for (const action of ["cancel", "confirm"]) {
    const f = harness();
    const id = await f.start();
    await f.poll(operation(id, "failed", required() + restored));
    assert.deepEqual(f.dialogs, [{ operationId: id, config: proposal(), binding }]);
    assert.equal(f.monitor.isPending(), false);
    assert.equal(f.flow.isPending(), true);
    assert.equal(f.calls.length, 1, "opening the dialog must not retry Save");
    assert.equal(f.flow.confirm("0"), null, "authorization belongs to the accepted operation");
    if (action === "cancel") {
      assert.deepEqual(f.flow.cancel(id), proposal());
      assert.equal(f.calls.length, 1);
      assert.equal(f.flow.confirm(id), null);
    } else {
      const request = f.flow.confirm(id);
      assert.deepEqual(request, { config: proposal(), authorization: { confirmed: true, binding } });
      assert.equal(f.flow.confirm(id), null);
      await f.start(request.config, request.authorization);
      assert.deepEqual(f.calls[1], { command: "save_config", args: {
        newConfig: proposal(), queueClearAuthorization: { confirmed: true, binding },
      } });
      assert.equal(f.calls.length, 2);
    }
    f.dispose();
  }
});

test("cancelling sends no Save or queue command and preserves an editable proposal", async () => {
  const f = harness(), config = proposal();
  const id = await f.start(config);
  await f.poll(operation(id, "failed", required()));
  const draft = f.flow.cancel(id);
  assert.deepEqual(draft, config);
  assert.equal(f.flow.isPending(), false);
  assert.equal(f.flow.confirm(id), null);
  assert.equal(f.calls.length, 1);
  draft.local_folders.push("/tmp/another-fixture");
  const nextId = await f.start(draft);
  assert.equal("queueClearAuthorization" in f.calls[1].args, false);
  await f.poll(operation(nextId, "failed", required(nextBinding)));
  assert.equal(f.dialogs.at(-1).binding, nextBinding);
  f.dispose();
});

test("confirm sends the exact backend binding and original proposed settings once", async () => {
  const f = harness(), config = proposal();
  const id = await f.start(config);
  config.local_folders.push("/tmp/late-edit");
  await f.poll(operation(id, "failed", required()));
  const request = f.flow.confirm(id);
  assert.deepEqual(request, { config: proposal(), authorization: { confirmed: true, binding } });
  assert.equal(f.flow.confirm(id), null);
  const nextId = await f.start(request.config, request.authorization);
  assert.deepEqual(f.calls[1], { command: "save_config", args: {
    newConfig: proposal(), queueClearAuthorization: { confirmed: true, binding },
  } });
  assert.ok(f.monitor.isPending());
  assert.equal(f.calls.length, 2);
  await f.poll(operation(nextId, "completed"));
  assert.equal(f.flow.isPending(), false);
  f.dispose();
});

test("stale confirmation never retries and requires a new unconfirmed check and new dialog", async () => {
  const f = harness();
  const firstId = await f.start();
  await f.poll(operation(firstId, "failed", required()));
  const request = f.flow.confirm(firstId);
  const secondId = await f.start(request.config, request.authorization);
  await f.poll(operation(secondId, "failed", "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE"));
  assert.equal(f.flow.isPending(), false);
  assert.equal(f.dialogs.length, 1);
  assert.equal(f.calls.length, 2);
  assert.equal(f.flow.confirm(firstId), null);
  const thirdId = await f.start();
  assert.equal("queueClearAuthorization" in f.calls[2].args, false);
  await f.poll(operation(thirdId, "failed", required(nextBinding)));
  assert.equal(f.flow.cancel(firstId), null);
  assert.equal(f.flow.confirm(firstId), null);
  assert.equal(f.flow.confirm(thirdId).authorization.binding, nextBinding);
  f.dispose();
});

test("uncertain, ordinary failure and historical refusal never grant confirmation", async () => {
  for (const [phase, error] of [["uncertain", required()], ["uncertain", required() + restored],
    ["failed", "fixture failure"], ["completed", required()], ["completed", required() + restored]]) {
    const f = harness();
    const id = await f.start();
    await f.poll(operation(id, phase, error));
    assert.equal(f.dialogs.length, 0);
    assert.equal(f.flow.confirm(id), null);
    f.dispose();
  }
  const flow = createQueueClearConfirmationFlow();
  assert.equal(flow.observe(operation("10", "failed", required())), null);
  assert.equal(flow.confirm("10"), null);
});

test("in-progress or malformed diagnostic cannot create or invent authorization", () => {
  for (const error of [null, required("x".repeat(64)), required("a".repeat(63)), required("a".repeat(65)),
    `prefix ${required()}`, `${required()}; cleanup ambiguous`, JSON.stringify(required()), required(binding.toUpperCase()),
    required("x".repeat(64)) + restored, required("a".repeat(63)) + restored,
    required("a".repeat(65)) + restored, required(binding.toUpperCase()) + restored,
    `prefix ${required()}${restored}`, `${required()}. Rollback também falhou: fixture failure`,
    `${required()}${restored} extra`, required() + restored + restored,
    `${required()}. Estado anterior restaurado`, `${required()} Estado anterior restaurado.`,
    `${required()}\n`, `${required()}${restored}\n`]) {
    assert.equal(queueClearBinding(error), null);
    const flow = createQueueClearConfirmationFlow(); flow.begin(proposal()); flow.accepted("1");
    assert.equal(flow.observe(operation("1", "failed", error)), null);
  }
  const flow = createQueueClearConfirmationFlow(); flow.begin(proposal()); flow.accepted("1");
  assert.equal(flow.observe(operation("1", "indexing", required())), null);
  assert.equal(flow.confirm("1"), null);
  assert.equal(flow.observe(operation("1", "failed", required())).binding, binding);
});

test("newer operation invalidates old proposal and late snapshots cannot reopen a dialog", async () => {
  const f = harness();
  const firstId = await f.start();
  await f.poll(operation(firstId, "failed", required()));
  f.flow.cancel(firstId);
  const secondId = await f.start();
  await f.poll(operation(firstId, "failed", required()));
  assert.equal(f.dialogs.length, 1);
  assert.equal(f.monitor.isPending(), true);
  await f.poll(operation(secondId, "failed", required(nextBinding)));
  assert.equal(f.dialogs.at(-1).operationId, secondId);
  f.flow.observe(operation("3", "indexing"));
  assert.equal(f.flow.confirm(secondId), null);
  assert.equal(f.flow.observe(operation(secondId, "failed", required(nextBinding))), null);
  f.dispose();
});

test("disposal prevents an in-flight terminal read from offering confirmation", async () => {
  const f = harness();
  const id = await f.start();
  f.dispose();
  await f.settleRead(operation(id, "failed", required()));
  assert.equal(f.dialogs.length, 0);
  assert.equal(f.timers.size, 0);
  assert.equal(f.flow.confirm(id), null);
});

function appConfirmationHandlers(flow) {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const source = app.slice(app.indexOf("const cancelQueueClear"), app.indexOf("// Carga inicial de configurações e dispositivos"));
  const effects = { opened: false, page: "plex", confirmation: "pending", saves: [] };
  const handlers = new Function("configSaveInProgressRef", "queueClearFlowRef", "setQueueClearConfirmation",
    "setShowSettings", "setSettingsInitialPage", "startLocalFoldersSave", stripTypeScriptTypes(source) + "\nreturn { cancelQueueClear, confirmQueueClear };")(
    { current: false }, { current: flow }, value => { effects.confirmation = value; },
    value => { effects.opened = value; }, value => { effects.page = value; }, (config, authorization) => { effects.saves.push({ config, authorization }); },
  );
  return { ...handlers, effects };
}

test("App cancellation discards the removal proposal and reopens Settings without another Save", () => {
  const flow = createQueueClearConfirmationFlow();
  flow.begin(proposal()); flow.accepted("1");
  flow.observe(operation("1", "failed", required() + restored));
  const app = appConfirmationHandlers(flow);
  app.cancelQueueClear("0");
  assert.equal(app.effects.opened, false, "stale cancellation cannot affect another operation");
  app.cancelQueueClear("1");
  assert.equal(app.effects.opened, true);
  assert.equal(app.effects.page, "general");
  assert.equal(app.effects.confirmation, null);
  assert.deepEqual(app.effects.saves, []);
  assert.equal(flow.isPending(), false);
  app.confirmQueueClear("1");
  assert.deepEqual(app.effects.saves, [], "cancelled authorization cannot be reused");
});

test("App confirmation still submits the exact proposal and backend authorization only once", () => {
  const flow = createQueueClearConfirmationFlow();
  flow.begin(proposal()); flow.accepted("1");
  flow.observe(operation("1", "failed", required() + restored));
  const app = appConfirmationHandlers(flow);
  app.confirmQueueClear("0");
  assert.deepEqual(app.effects.saves, []);
  app.confirmQueueClear("1");
  app.confirmQueueClear("1");
  assert.deepEqual(app.effects.saves, [{ config: proposal(), authorization: { confirmed: true, binding } }]);
  assert.equal(app.effects.opened, false);
});

test("Settings Cancel, X and Escape unmount edits and reopening loads persisted config", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const modal = readFileSync(new URL("../components/SettingsModal.tsx", import.meta.url), "utf8");
  // Overlap refusal alone may restore a draft; explicit cancellation must clear it.
  assert.match(app, /onClose=\{\(\) => \{ setShowSettings\(false\); setLocalFoldersDraft\(null\); \}\}/);
  assert.match(app, /else if \(showSettings\) \{\s*setShowSettings\(false\);\s*setLocalFoldersDraft\(null\)/);
  assert.match(app, /e\.key === "Escape"[\s\S]*?else if \(showSettings\) \{\s*setShowSettings\(false\)/);
  assert.match(app, /onClick=\{\(\) => \{ if \(!configSaveInProgressRef\.current\) \{ setSettingsInitialPage\("general"\); setShowSettings\(true\); \} \}\}/);
  assert.match(modal, /Promise\.all\(\[configService\.getConfig\(\), configService\.getAudioDevices\(\)\]\)/);
  assert.match(modal, /setConfig\(initialDraft \?[^;]+: cfg\)/);
  const buttons = [...modal.matchAll(/<button\b[^>]*onClick=\{onClose\}[^>]*>([\s\S]*?)<\/button>/g)].map(match => match[1]);
  assert.ok(buttons.some(content => content.includes("<X ")), "X uses the unmounting close callback");
  assert.ok(buttons.some(content => content.includes('t("settings.cancel")')), "Cancel uses the same callback");
});

test("confirmation wiring discards cancelled edits, uses an accessible dialog and translates every message", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const modal = readFileSync(new URL("../components/SettingsModal.tsx", import.meta.url), "utf8");
  assert.match(app, /queueClearFlowRef\.current\.accepted\(response\.operation\.id\);\s*monitor\.accepted/);
  assert.match(app, /startLocalFoldersSave\(request\.config, request\.authorization\)/);
  const cancel = app.slice(app.indexOf("const cancelQueueClear"), app.indexOf("const confirmQueueClear"));
  assert.doesNotMatch(cancel, /saveConfig|startLocalFoldersSave|setConfig\(|setLocalFoldersDraft/);
  assert.match(cancel, /queueClearFlowRef\.current\.cancel\(operationId\)/);
  assert.match(cancel, /setQueueClearConfirmation\(null\);\s*setSettingsInitialPage\("general"\);\s*setShowSettings\(true\)/);
  assert.match(modal, /role="alertdialog"[\s\S]*?aria-modal="true"[\s\S]*?aria-labelledby=\{titleId\}[\s\S]*?aria-describedby=\{descriptionId\}/);
  assert.match(modal, /cancelRef\.current\?\.focus\(\)/);
  assert.match(modal, /event\.key === "Escape"/);
  assert.match(modal, /event\.key === "Tab"/);
  assert.doesNotMatch(app + modal, /queueClearConfirmation\.binding|audioService\.clearQueue/);
  for (const locale of ["en", "pt-BR"]) {
    const { settings } = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    for (const key of ["queueClearTitle", "queueClearDescription", "queueClearCancel", "queueClearConfirm", "queueClearStale", "queueClearCheckRequired", "queueClearRecheck"]) assert.ok(settings[key]);
    assert.match(settings.queueClearDescription, locale === "en" ? /interrupt playback.*ENTIRE queue/ : /reprodução será interrompida.*TODA a fila/);
    assert.match(settings.queueClearDescription, locale === "en" ? /discard the proposed changes.*reopen saved preferences/ : /descarta as alterações propostas.*reabre as preferências salvas/);
  }
});

test("removal progress keeps its proposal until a real terminal result", () => {
  const flow=createQueueClearConfirmationFlow();
  flow.begin({local_folders:["/own-fixture/keep"]});flow.accepted("9");
  assert.equal(flow.observe({id:"9",phase:"removing",error:null}),null);
  const prompt=flow.observe({id:"9",phase:"failed",error:required()});
  assert.equal(prompt.binding,binding);assert.deepEqual(prompt.config.local_folders,["/own-fixture/keep"]);
});
