import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createDeviceAvailabilityQuery, newPhysicalOutputChoice, shouldCheckContextualDevice } from "./deviceAvailability.ts";

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

test("availability states and momentary limitations are translated", () => {
  for (const language of ["en", "pt-BR"]) {
    const labels = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8")).settings.deviceAvailability;
    for (const key of ["check", "checking", "available", "busy", "indeterminate", "current_sonante", "failed", "notice", "busyAction",
      "helpLabel", "helpObservation", "helpShared", "helpBusy", "helpLimits"]) {
      assert.ok(typeof labels[key] === "string" && labels[key].length > 0);
    }
    assert.match(labels.notice, language === "en" ? /do not reserve/ : /Não reservam/);
    assert.equal(labels.available, language === "en" ? "Available" : "Disponível");
    assert.equal(labels.busy, language === "en" ? "In use" : "Em uso");
    assert.equal(labels.indeterminate, language === "en" ? "Not checked" : "Não verificado");
    assert.match(labels.helpObservation, language === "en" ? /last check/ : /última verificação/);
    assert.match(labels.helpShared, /Shared.*PipeWire.*PulseAudio/);
    assert.match(labels.helpBusy, language === "en" ? /not necessarily/ : /não significa necessariamente/);
    assert.match(labels.helpLimits, language === "en" ? /does not reserve.*audio formats/ : /não reserva.*formatos de áudio/);
  }
});

test("each explicit request targets one endpoint and preserves all safe states", async () => {
  for (const state of ["available", "busy", "indeterminate", "current_sonante"]) {
    const request = deferred(), changes = [], ids = [];
    const query = createDeviceAvailabilityQuery(id => { ids.push(id); return request.promise; }, state => changes.push(state));
    assert.deepEqual(ids, []); assert.equal(changes.length, 0);
    const pending = query.check("dac");
    assert.deepEqual(ids, ["dac"]); assert.equal(changes.at(-1).checking, true);
    request.resolve([{ id: "dac", state }]);
    assert.deepEqual(await pending, { id: "dac", state });
    assert.deepEqual(changes.at(-1), { checking: false, results: [{ id: "dac", state }], failed: false });
    query.dispose();
  }
});

test("errors, missing endpoints and invalid states become indeterminate without raw diagnostics", async () => {
  for (const read of [async () => { throw new Error("/private/SECRET https://user:password.invalid/?X-Plex-Token=SECRET"); },
    async () => [], async () => [{ id: "other", state: "available" }], async () => [{ id: "dac", state: "RAW_SECRET" }]]) {
    const changes = [];
    const query = createDeviceAvailabilityQuery(read, state => changes.push(state));
    await query.check("dac");
    assert.deepEqual(changes.at(-1), { checking: false, results: [{ id: "dac", state: "indeterminate" }], failed: true });
    assert.ok(!JSON.stringify(changes).includes("SECRET")); query.dispose();
  }
});

test("rapid selections serialize I/O, retain only the latest endpoint and discard late success or failure", async () => {
  for (const fails of [false, true]) {
    const first = deferred(), last = deferred(), changes = [], ids = [];
    const query = createDeviceAvailabilityQuery(id => { ids.push(id); return ids.length === 1 ? first.promise : last.promise; }, state => changes.push(state));
    const old = query.check("A"), skipped = query.check("B"), current = query.check("C");
    assert.deepEqual(ids, ["A"]);
    assert.equal(await old, null); assert.equal(await skipped, null);
    const count = changes.length;
    if (fails) first.reject(new Error("SECRET")); else first.resolve([{ id: "A", state: "available" }]);
    await flush();
    assert.deepEqual(ids, ["A", "C"]); assert.equal(changes.length, count);
    last.resolve([{ id: "C", state: "busy" }]); await current;
    assert.deepEqual(changes.at(-1).results, [{ id: "C", state: "busy" }]); query.dispose();
  }
});

test("duplicate pending checks share one IPC; explicit retry gets a fresh result", async () => {
  const requests = [], ids = [], changes = [];
  const query = createDeviceAvailabilityQuery(id => { const d = deferred(); requests.push(d); ids.push(id); return d.promise; }, state => changes.push(state));
  const first = query.check("dac"), duplicate = query.check("dac");
  assert.equal(first, duplicate); assert.deepEqual(ids, ["dac"]);
  requests[0].resolve([{ id: "dac", state: "busy" }]); await first; await flush();
  const retry = query.check("dac");
  assert.deepEqual(ids, ["dac", "dac"]);
  requests[1].resolve([{ id: "dac", state: "available" }]); await retry;
  assert.equal(changes.at(-1).results[0].state, "available"); query.dispose();
});

test("selection/mode invalidation cancels queued work and cannot publish obsolete results", async () => {
  const request = deferred(), changes = [], ids = [];
  const query = createDeviceAvailabilityQuery(id => { ids.push(id); return request.promise; }, state => changes.push(state));
  const first = query.check("A"), pending = query.check("B"); query.invalidate();
  assert.equal(await first, null); assert.equal(await pending, null);
  const count = changes.length;
  request.resolve([{ id: "A", state: "available" }]); await flush();
  assert.equal(changes.length, count); assert.deepEqual(ids, ["A"]);
  assert.deepEqual(changes.at(-1), { checking: false, results: [], failed: false }); query.dispose();
});

test("closing Settings discards active and queued results and prevents further probes", async () => {
  const request = deferred(), changes = [], ids = [];
  const query = createDeviceAvailabilityQuery(id => { ids.push(id); return request.promise; }, state => changes.push(state));
  const first = query.check("A"), second = query.check("B"); query.dispose();
  assert.equal(await first, null); assert.equal(await second, null);
  request.reject(new Error("SECRET")); await flush(); await query.check("C");
  assert.deepEqual(ids, ["A"]); assert.equal(changes.length, 2);
});

test("busy visual status may block only a new physical Direct choice, never unrelated settings", () => {
  const original = { audio_output_type: "alsa", alsa_device: "hw:A,0" };
  assert.equal(newPhysicalOutputChoice(original, { ...original, audio_buffer_size_kb: 32768, replay_gain: "album" }), false);
  assert.equal(newPhysicalOutputChoice(original, { ...original, alsa_device: "hw:B,0" }), true);
  for (const id of ["default", "null", "custom", "plughw:0,0"]) assert.equal(newPhysicalOutputChoice(original, { ...original, alsa_device: id }), false);
  assert.equal(newPhysicalOutputChoice(original, { ...original, audio_output_type: "pipewire" }), false);
});

test("contextual checks exclude the saved Direct owner, virtual and unenumerated choices without guessing alias identity", () => {
  const original = { first_run: false, audio_output_type: "alsa", alsa_device: "hw:A,0" };
  const enumerated = ["hw:A,0", "hw:B,0", "null", "default"];
  assert.equal(shouldCheckContextualDevice(original, original, enumerated), false);
  assert.equal(shouldCheckContextualDevice({ ...original, audio_output_type: "pipewire" }, original, enumerated), true);
  assert.equal(shouldCheckContextualDevice({ ...original, first_run: true }, original, enumerated), true);
  assert.equal(shouldCheckContextualDevice(original, { ...original, alsa_device: "hw:B,0" }, enumerated), true);
  for (const id of ["null", "default", "plughw:A,0", "hw:Unknown,0"]) {
    assert.equal(shouldCheckContextualDevice(original, { ...original, alsa_device: id }, enumerated), false);
  }
  assert.equal(shouldCheckContextualDevice(original, { ...original, audio_output_type: "pipewire" }, enumerated), false);
  // Enumerated aliases still pass through the existing backend identity checks;
  // the frontend never decides whether they are safe to open.
  assert.equal(shouldCheckContextualDevice(original, { ...original, alsa_device: "hw:0,0" }, ["hw:0,0"]), true);
});

test("IPC is directed and backend enumeration/admission precede the injected-safe classifier", () => {
  const service = readFileSync(new URL("../services/config.ts", import.meta.url), "utf8");
  const backend = readFileSync(new URL("../../src-tauri/src/lib.rs", import.meta.url), "utf8");
  assert.match(service, /checkDirectDeviceAvailability: \(deviceId: string\)[\s\S]*?"check_direct_device_availability", \{ deviceId \}/);
  const command = backend.slice(backend.indexOf("async fn check_direct_device_availability("), backend.indexOf("async fn plex_create_pin"));
  assert.match(command, /device_id: String/); assert.match(command, /spawn_blocking/);
  const helper = backend.slice(backend.indexOf("fn check_direct_device_availability_with("), backend.indexOf("async fn check_direct_device_availability("));
  assert.match(helper, /\.begin\(\)/);
  assert.ok(helper.indexOf(".begin()") < helper.indexOf("let devices = enumerate()"));
  assert.match(helper, /availability_for_with\(device_id, &devices, current_direct/);
  assert.match(command, /check_direct_device_availability_with\(\s*&device_id/);
});
