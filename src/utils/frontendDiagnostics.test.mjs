import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import ts from "typescript";
import { logError, logWarning, summarizeDiagnostic } from "./frontendDiagnostics.ts";
import { exploreDiagnosticAlias, exploreDiagnosticText } from "./frontendDiagnostics.ts";

const secrets = "https://user:password@server/private/song?X-Plex-Token=secret /home/private/music /mnt/private";

test("real Plex mix helper is DEV-only without opt-in, bounds scalars and tolerates a broken console", async () => {
  const source = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  const previous = console.info, calls = [];
  console.info = (...args) => calls.push(args);
  try {
    for (const env of [{ DEV: false, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "1" }, { DEV: true }, { DEV: true, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "0" }, { DEV: true, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "1" }]) {
      const js = ts.transpileModule(source.replaceAll("import.meta.env", JSON.stringify(env)), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
      const { logPlexMixDiagnostic: log } = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
      const before = calls.length;
      log("ipc", "PLEX_DISCOVER_UNAVAILABLE", "operation_rejected", 0);
      assert.equal(calls.length - before, env.DEV ? 1 : 0);
      if (!env.DEV) continue;
      for (const [value, expected] of [[999, 101], [-2, 0], [3.8, 3], [NaN, 0], [Infinity, 0], [secrets, 0]]) {
        log("frontend", "PLEX_DISCOVER_INVALID", "response_shape", value);
        assert.deepEqual(calls.at(-1), ["[Sonante] plex.mix.frontend", JSON.stringify({ stage: "frontend", category: "PLEX_DISCOVER_INVALID", reason: "response_shape", observed: expected })]);
      }
      const count = calls.length;
      for (const args of [[secrets, "PLEX_DISCOVER_INVALID", "response_shape", 1], ["ipc", secrets, "operation_rejected", 1], ["frontend", "PLEX_DISCOVER_INVALID", secrets, 1]]) log(...args);
      assert.equal(calls.length, count, "unknown enum values must not be logged");
      console.info = () => { throw Error(secrets); };
      assert.doesNotThrow(() => log("frontend", "PLEX_DISCOVER_CONTEXT", "track_or_locator_scope", 1));
      console.info = (...args) => calls.push(args);
    }
  } finally { console.info = previous; }
  assert.doesNotMatch(JSON.stringify(calls), /secret|password|https:|private|X-Plex/);
  for (const [, payload] of calls) assert.deepEqual(Object.keys(JSON.parse(payload)), ["stage", "category", "reason", "observed"]);
});

test("thumbnail diagnostics stay DEV-only and allow only fixed reasons and numeric counters", async () => {
  const source = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  const previous = console.info, calls = [];
  console.info = (...args) => calls.push(args);
  try {
    for (const env of [{ DEV: false }, { DEV: false, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "1" }, { DEV: true }, { DEV: true, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "0" }]) {
      const js = ts.transpileModule(source.replaceAll("import.meta.env", JSON.stringify(env)), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
      const { logLocalThumbnailDiagnostic: log } = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
      const value = { stage: "summary", restore: "restored", restored: 2, captured: 3, stored: 3, retained: 2, writes: 1, write_failures: 0,
        on_load: 2, already_complete: 1, initial_cards: 2, initial_hits: 1, initial_misses: 1, evicted_count: 1, evicted_bytes: 500,
        rejected: { admission: 1, canvas_failed: 2 } };
      const before = calls.length;
      log(value);
      assert.equal(calls.length - before, env.DEV ? 1 : 0);
      if (!env.DEV) continue;
      assert.deepEqual(calls.at(-1), ["[Sonante] local.thumbnails", JSON.stringify(value)]);
      for (const invalid of [secrets, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
        log({ ...value, captured: invalid, rejected: { ...value.rejected, admission: invalid, [secrets]: 9 }, path: secrets, image: secrets });
        const payload = JSON.parse(calls.at(-1)[1]);
        assert.equal(payload.captured, 0); assert.equal(payload.rejected.admission, 0);
        assert.deepEqual(Object.keys(payload), Object.keys(value));
        assert.deepEqual(Object.keys(payload.rejected), ["admission", "canvas_failed"]);
      }
      const count = calls.length;
      for (const invalid of [null, undefined, { ...value, stage: secrets }, { ...value, restore: secrets },
        { get stage() { throw Error(secrets); } }, new Proxy({}, { getOwnPropertyDescriptor() { throw Error(secrets); } })]) {
        assert.doesNotThrow(() => log(invalid));
      }
      assert.equal(calls.length, count);
      console.info = () => { throw Error(secrets); };
      assert.doesNotThrow(() => log(value));
      console.info = (...args) => calls.push(args);
    }
  } finally { console.info = previous; }
  assert.doesNotMatch(JSON.stringify(calls), /secret|password|https:|private|X-Plex/);
});

test("Explore diagnostics require both development and explicit opt-in, build lazily and tolerate failure", async () => {
  const source = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  const old = console.info, calls = []; let built = 0;
  console.info = (...args) => calls.push(args);
  try {
    for (const env of [{ DEV: false, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "1" }, { DEV: true }, { DEV: true, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "0" }, { DEV: true, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "1" }]) {
      const js = ts.transpileModule(source.replaceAll("import.meta.env", JSON.stringify(env)), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
      const helper = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
      helper.logExploreDiagnostic(() => { built++; return { count: 6 }; });
      assert.doesNotThrow(() => helper.logExploreDiagnostic(() => { throw Error(secrets); }));
    }
  } finally { console.info = old; }
  assert.equal(built, 1); assert.deepEqual(calls, [["[Sonante] explore.diagnostic", '{"count":6}']]);
});

test("Explore metadata and identifiers omit paths, URLs and credential-like values", () => {
  for (const value of [secrets, "/mnt/music", "C:\\Music", "token=private", "password:private", "secret=private", "file:private", "bad\nvalue"]) assert.equal(exploreDiagnosticText(value), "[redacted]");
  assert.equal(exploreDiagnosticText("Brasilidades"), "Brasilidades");
  assert.equal(exploreDiagnosticText("a".repeat(100)).length, 80);
  const alias = exploreDiagnosticAlias("root", secrets);
  assert.match(alias, /^root-\d+$/); assert.equal(exploreDiagnosticAlias("root", secrets), alias);
  assert.notEqual(exploreDiagnosticAlias("root", "other"), alias);
  assert.match(exploreDiagnosticAlias("library", secrets), /^library-\d+$/);
});

test("strings and Error instances retain safe ACK/OS context without secrets", () => {
  for (const error of [`ACK [5@2] {add} ${secrets} (os error 13)`, new Error(`ACK [5@2] {add} ${secrets} (os error 13)`)]) {
    const result = summarizeDiagnostic(error);
    assert.equal(result.mpd_ack_code, 5);
    assert.equal(result.mpd_command_index, 2);
    assert.equal(result.os_code, 13);
    assert.doesNotMatch(JSON.stringify(result), /secret|password|https:|private|X-Plex|stack|message/);
    assert.ok(Object.isFrozen(result));
  }
});

test("known codes survive, including confirmation code without its private binding", () => {
  const code = "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED";
  assert.deepEqual(summarizeDiagnostic(`${code}:${"a".repeat(64)}. Estado anterior restaurado.`), { error_type: "string", code });
  assert.deepEqual(summarizeDiagnostic({ code: "durability_failed", committed: true, status: 503, message: secrets, token: "secret" }),
    { error_type: "object", code: "durability_failed", http_status: 503, committed: true });
  assert.equal(summarizeDiagnostic({ code: "EACCES" }).code, "EACCES");
  assert.equal(summarizeDiagnostic("playlist_no_playable_items").code, "playlist_no_playable_items");
  assert.equal(summarizeDiagnostic({ code: "secret" }).code, undefined);
  assert.equal(summarizeDiagnostic(`UNKNOWN_CODE:${secrets}`).code, undefined);
  assert.equal(summarizeDiagnostic({ status: 999 }).http_status, undefined);
});

test("unexpected values, circular objects, accessors and hostile proxies are safe", () => {
  const circular = { token: "secret" }; circular.self = circular;
  const accessor = { get message() { assert.fail("must not execute getters"); }, toJSON() { assert.fail("must not call toJSON"); }, toString() { assert.fail("must not call toString"); } };
  const proxy = new Proxy({}, { getOwnPropertyDescriptor() { throw new Error(secrets); } });
  const values = [undefined, null, 123, true, 9n, Symbol(secrets), () => {}, circular, accessor, proxy];
  for (const error of values) {
    assert.doesNotThrow(() => JSON.stringify(summarizeDiagnostic(error)));
    assert.doesNotMatch(JSON.stringify(summarizeDiagnostic(error)), /secret|private|password|https:/);
  }
  assert.equal(summarizeDiagnostic(proxy).inspection_failed, true);
  assert.deepEqual(summarizeDiagnostic("ACK [secret@0] (os error token)"), { error_type: "string" });
});

test("Plex HTTP failures retain only their numeric status, independent of UI language", () => {
  for (const message of [`Plex authentication failed (HTTP 401). ${secrets}`, `O servidor Plex falhou (HTTP 503). ${secrets}`]) {
    const result = summarizeDiagnostic(new Error(message));
    assert.equal(result.http_status, message.includes("401") ? 401 : 503);
    assert.doesNotMatch(JSON.stringify(result), /secret|https:|password|private/);
  }
  assert.equal(summarizeDiagnostic("HTTP 999").http_status, undefined);
});

test("log levels and operation context are preserved without raw external data", () => {
  const oldError = console.error, oldWarn = console.warn, calls = [];
  console.error = (...args) => calls.push(["error", ...args]);
  console.warn = (...args) => calls.push(["warn", ...args]);
  try {
    logError("settings.save", new Error(`ALSA_PREFLIGHT_OUTPUT_BUSY: ${secrets}`));
    logWarning("playlists.item-unavailable", { code: "unavailable", message: secrets });
    logError(secrets, { arbitrary: secrets });
  } finally { console.error = oldError; console.warn = oldWarn; }
  assert.deepEqual(calls[0], ["error", "[Sonante] operation=settings.save", { error_type: "object", code: "ALSA_PREFLIGHT_OUTPUT_BUSY" }]);
  assert.equal(calls[1][0], "warn");
  assert.equal(calls[2][1], "[Sonante] operation=unknown-operation");
  assert.doesNotMatch(JSON.stringify(calls), /secret|password|https:|private/);
});

test("English text hints are separate from UI errors, which remain unmodified", () => {
  const error = Object.freeze({ code: "read_failed", message: `Serviço indisponível ${secrets}` });
  assert.equal(summarizeDiagnostic(error).hint, "unavailable");
  assert.equal(error.message, `Serviço indisponível ${secrets}`);
  assert.equal(summarizeDiagnostic("tempo limite").hint, "timeout");
  assert.equal(summarizeDiagnostic("Permission denied").hint, "permission_denied");
});

test("production frontend console calls are centralized and every operation is a static allowlisted literal", () => {
  const root = new URL("../", import.meta.url);
  const helper = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  const allowed = new Set(JSON.parse(helper.match(/const operations = (\[[\s\S]*?\]) as const/)[1]));
  let checked = 0;
  function walk(dir) {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const file = new URL(entry.name + (entry.isDirectory() ? "/" : ""), dir);
      if (entry.isDirectory()) { walk(file); continue; }
      if (!/\.tsx?$/.test(entry.name) || entry.name === "frontendDiagnostics.ts") continue;
      const source = readFileSync(file, "utf8");
      const tree = ts.createSourceFile(file.pathname, source, ts.ScriptTarget.Latest, true, entry.name.endsWith("tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
      function visit(node) {
        if (ts.isPropertyAccessExpression(node) && node.expression.getText(tree) === "console") assert.fail(`Direct console access in ${entry.name}`);
        if (ts.isElementAccessExpression(node) && node.expression.getText(tree) === "console") assert.fail(`Direct console access in ${entry.name}`);
        if (ts.isCallExpression(node) && ["logError", "logWarning"].includes(node.expression.getText(tree))) {
          const context = node.arguments[0];
          assert.ok(context && ts.isStringLiteral(context), `Dynamic diagnostic context in ${entry.name}`);
          assert.ok(allowed.has(context.text), `Unknown operation ${context.text}`);
          checked++;
        }
        ts.forEachChild(node, visit);
      }
      visit(tree);
    }
  }
  walk(root);
  assert.ok(checked > 60);
});

test("interaction observations aggregate healthy IPC, retain pending work and bound histories with safe correlation", async () => {
  const { createInteractionObservations } = await import("./frontendDiagnostics.ts");
  let time = 0;
  const observer = createInteractionObservations(() => time);
  for (let i = 0; i < 10000; i++) { const id = observer.begin("get_mpd_status_snapshot"); observer.finish(id, false); }
  let snapshot = observer.snapshot();
  assert.equal(snapshot.counts.get_mpd_status_snapshot.completed, 10000);
  assert.deepEqual(snapshot.recent, []);
  observer.action("local", "entered"); observer.action("local", "returned");
  const pending = observer.begin("get_favorites"); time = 20000;
  snapshot = observer.snapshot();
  assert.equal(snapshot.pending[0].ipc, pending);
  assert.equal(snapshot.pending[0].action, snapshot.recent[0].action);
  assert.equal(snapshot.pending[0].elapsed_ms, 20000);
  assert.equal(observer.snapshot().active_count, 1, "reporting is not cancellation");
  observer.finish(pending, true, new Error(secrets));
  assert.equal(observer.snapshot().counts.get_favorites.failed, 1);
  for (let i = 0; i < 1000; i++) observer.begin("get_local_cover");
  snapshot = observer.snapshot();
  assert.equal(snapshot.active_count, 128); assert.equal(snapshot.pending.length, 16); assert.equal(snapshot.omitted, 872);
  for (let i = 0; i < 10000; i++) observer.input("pointer", true, false);
  observer.state({ section: secrets, health: secrets, settings: true, catalogAlbums: 908 });
  observer.action(secrets, "entered"); assert.equal(observer.begin(secrets), undefined);
  snapshot = observer.snapshot(); assert.equal(snapshot.recent.length, 16);
  assert.equal(snapshot.recent.at(-1).event, "react.commit"); assert.equal(snapshot.recent.at(-1).section, null);
  assert.doesNotMatch(JSON.stringify(snapshot), /secret|password|https:|private|X-Plex/);
});

test("IPC diagnostics pass safe identifiers only under opt-in and preserve values, rejections and executing promises", async () => {
  const source = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  for (const enabled of [false, true]) {
    const env = { DEV: enabled, VITE_SONANTE_EXPLORE_DIAGNOSTICS: "1" };
    const js = ts.transpileModule(source.replaceAll("import.meta.env", JSON.stringify(env)), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
    const helper = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
    const calls = [], value = { preserved: secrets };
    const invoke = async (command, args) => { calls.push({ command, args }); return value; };
    assert.equal(await helper.diagnosticInvoke(invoke, "get_favorites"), value);
    assert.equal(typeof calls[0].args?.diagnosticId, enabled ? "number" : "undefined");
    await helper.diagnosticInvoke(invoke, "get_local_cover", { path: secrets });
    assert.equal(calls[1].args.path, secrets, "real command arguments remain intact and are never logged");
    if (enabled) assert.notEqual(calls[0].args.diagnosticId, calls[1].args.diagnosticId);
    await helper.diagnosticInvoke(invoke, "untracked_command", { token: secrets });
    assert.deepEqual(calls[2].args, { token: secrets });
    const error = new Error(secrets);
    await assert.rejects(helper.diagnosticInvoke(async () => { throw error; }, "get_favorites"), e => e === error);
    assert.throws(() => helper.diagnosticInvoke(() => { throw error; }, "get_favorites"), e => e === error);
    let resolve, settled = false;
    const original = new Promise(done => { resolve = done; });
    const pending = helper.diagnosticInvoke(() => original, "get_favorites");
    pending.then(() => { settled = true; });
    for (let i = 0; i < 50; i++) await Promise.resolve();
    assert.equal(settled, false); resolve(value); assert.equal(await pending, value);
  }
});

test("Explore diagnostic bursts and retained aliases are bounded", async () => {
  const source = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  const js = ts.transpileModule(source.replaceAll("import.meta.env", '{"DEV":true,"VITE_SONANTE_EXPLORE_DIAGNOSTICS":"1"}'), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
  const helper = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
  const old = console.info, lines = []; let built = 0;
  console.info = (...args) => lines.push(args);
  try { for (let i = 0; i < 10000; i++) helper.logExploreDiagnostic(() => { built++; return { count: i }; }); }
  finally { console.info = old; }
  assert.ok(lines.length <= 16); assert.ok(built <= 16);
  for (let i = 0; i < 10000; i++) helper.exploreDiagnosticAlias("root", String(i));
  assert.equal(helper.exploreDiagnosticAlias("root", "another"), "root-omitted");
});

test("frontend probe reports loop lag, a blocked frame, overlay hit testing and pending IPC without cancelling work", async () => {
  const source = readFileSync(new URL("./frontendDiagnostics.ts", import.meta.url), "utf8");
  const js = ts.transpileModule(source.replaceAll("import.meta.env", '{"DEV":true,"VITE_SONANTE_EXPLORE_DIAGNOSTICS":"1"}'), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
  const helper = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}#probe`);
  const keys = ["window", "document", "performance", "requestAnimationFrame", "cancelAnimationFrame", "getComputedStyle", "Element"];
  const saved = Object.fromEntries(keys.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const old = console.info, logs = [], listeners = new Map(); let time = 0, tick, cancelled = 0, cleared = 0, resolve;
  const sidebar = { getBoundingClientRect: () => ({ left: 0, top: 0, width: 224, height: 600 }), contains: () => false, closest: () => null };
  const fake = {
    window: { setInterval: fn => { tick = fn; return 7; }, clearInterval: id => { assert.equal(id, 7); cleared++; } },
    document: { visibilityState: "visible", querySelector: () => sidebar, elementFromPoint: () => ({}), addEventListener: (name, fn) => listeners.set(name, fn), removeEventListener: name => listeners.delete(name) },
    performance: { now: () => time }, requestAnimationFrame: () => 9, cancelAnimationFrame: id => { assert.equal(id, 9); cancelled++; },
    getComputedStyle: () => ({ pointerEvents: "none" }), Element: class {},
  };
  for (const key of keys) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value: fake[key] });
  console.info = (...args) => logs.push(args);
  let cleanup;
  try {
    cleanup = helper.installInteractionProbe(() => ({ active: 2, queued: 27, inFlight: 29, blocked: false, path: secrets }));
    assert.equal(listeners.size, 2);
    helper.traceInteractionAction("local", () => {});
    helper.recordInteractionState({ section: "local", health: "available", catalogAlbums: 908, settings: false });
    const work = helper.diagnosticInvoke(() => new Promise(done => { resolve = done; }), "get_favorites");
    for (time = 1000; time <= 10000; time += 1000) tick();
    let summary = JSON.parse(logs.find(row => row[0] === "[Sonante] dev.frontend")[1]);
    assert.equal(summary.loop.ticks, 10); assert.equal(summary.loop.frame_pending_ms, 9000);
    assert.equal(summary.dom.sidebar_hit, false); assert.equal(summary.dom.sidebar_pointer_blocked, true);
    assert.equal(summary.active_count, 1); assert.equal(summary.pending[0].command, "get_favorites");
    assert.deepEqual(summary.artwork_queue, { active: 2, queued: 27, in_flight: 29, blocked: false });
    assert.doesNotMatch(JSON.stringify(summary), /private|secret|X-Plex/);
    assert.equal(summary.recent.find(row => row.event === "react.commit").catalogAlbums, 908);
    time = 25000; tick(); summary = JSON.parse(logs.at(-1)[1]);
    assert.equal(summary.loop.maximum_lag_ms, 14000); assert.equal(summary.active_count, 1);
    resolve("preserved"); assert.equal(await work, "preserved");
    cleanup(); cleanup = null; assert.equal(listeners.size, 0); assert.equal(cleared, 1); assert.equal(cancelled, 1);
  } finally {
    cleanup?.(); console.info = old;
    for (const key of keys) { if (saved[key]) Object.defineProperty(globalThis, key, saved[key]); else delete globalThis[key]; }
  }
});
