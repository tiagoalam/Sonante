import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import ts from "typescript";
import { logError, logWarning, summarizeDiagnostic } from "./frontendDiagnostics.ts";

const secrets = "https://user:password@server/private/song?X-Plex-Token=secret /home/private/music /mnt/private";

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
