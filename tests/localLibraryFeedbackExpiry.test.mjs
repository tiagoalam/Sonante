import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = readFileSync(new URL("../src/components/LocalBrowserView.tsx", import.meta.url), "utf8");
const file = ts.createSourceFile("LocalBrowserView.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let callback;
const visit = node => {
  if (ts.isCallExpression(node) && node.expression.getText(file) === "useEffect"
    && node.arguments[0]?.getText(file).includes("setCheckFeedback")) callback = node.arguments[0];
  ts.forEachChild(node, visit);
};
visit(file);
assert.ok(callback, "the real library feedback expiration effect exists");
const js = `return (${callback.getText(file)});`;

test("library-check feedback expires after 30 seconds and stale timers cannot clear newer feedback", () => {
  let scheduled, cancelled = false, current = { scope: "all", kind: "updated" };
  const window = {
    setTimeout(fn, delay) { assert.equal(delay, 30_000); scheduled = fn; return 7; },
    clearTimeout(id) { assert.equal(id, 7); cancelled = true; },
  };
  const setCheckFeedback = update => { current = update(current); };
  const cleanup = new Function("window", "checkFeedback", "setCheckFeedback", js)(
    window, current, setCheckFeedback,
  )();
  assert.equal(typeof scheduled, "function");
  current = { scope: "all", kind: "failed" };
  scheduled();
  assert.deepEqual(current, { scope: "all", kind: "failed" });
  cleanup();
  assert.equal(cancelled, true);
});

