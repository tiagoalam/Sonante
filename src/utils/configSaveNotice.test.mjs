import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const source = app.slice(app.indexOf("// Expire only visible success notices"), app.indexOf("const [showAbout"));
const effect = new Function("useEffect", "window", "configSaveNotice", "configSavePhase",
  "configSaveGenerationRef", "configSaveInProgressRef", "setConfigSaveNotice", source);

function harness() {
  let now = 0, cleanup;
  const timers = [];
  const state = { notice: null, phase: null, generation: { current: 1 }, pending: { current: false } };
  const clock = {
    setTimeout(callback, delay) { const timer = { callback, at: now + delay, cancelled: false }; timers.push(timer); return timer; },
    clearTimeout(timer) { timer.cancelled = true; },
  };
  const render = () => {
    cleanup?.();
    effect(callback => { cleanup = callback(); }, clock, state.notice, state.phase, state.generation, state.pending,
      update => { state.notice = typeof update === "function" ? update(state.notice) : update; });
  };
  return {
    state, timers, render,
    success() { state.notice = { kind: "success" }; render(); },
    close() { state.notice = null; render(); },
    begin() { state.generation.current++; state.pending.current = true; state.phase = "local"; state.notice = null; render(); },
    advance(ms) {
      now += ms;
      for (const timer of timers) if (!timer.cancelled && timer.at <= now) { timer.cancelled = true; timer.callback(); }
    },
    unmount() { cleanup?.(); },
  };
}

test("success disappears after 30 seconds of display, never earlier", () => {
  const f = harness(); f.success();
  f.advance(29_999); assert.equal(f.state.notice.kind, "success");
  f.advance(1); assert.equal(f.state.notice, null);
});

test("X dismisses immediately and cancels its timer", () => {
  const f = harness(); f.success(); f.close();
  assert.equal(f.state.notice, null); assert.equal(f.timers[0].cancelled, true);
  f.timers[0].callback(); assert.equal(f.state.notice, null);
  assert.match(app, /onClick=\{\(\) => setConfigSaveNotice\(null\)\} aria-label=\{t\("settings\.dismissStatus"\)\}/);
});

test("replacement success receives a fresh 30 seconds and ignores the older callback", () => {
  const f = harness(); f.success(); f.advance(20_000); const old = f.timers[0];
  f.success(); const next = f.state.notice;
  assert.equal(old.cancelled, true); old.callback(); assert.equal(f.state.notice, next);
  f.advance(10_000); assert.equal(f.state.notice, next);
  f.advance(20_000); assert.equal(f.state.notice, null);
});

test("new Save cancels expiration, protects progress and subsequent success", () => {
  const f = harness(); f.success(); const old = f.timers[0];
  // The generation/in-progress guards also protect the interval before effect cleanup.
  f.state.generation.current++; f.state.pending.current = true;
  old.callback(); assert.equal(f.state.notice.kind, "success");
  f.begin(); assert.equal(old.cancelled, true);
  f.advance(30_000); assert.equal(f.state.phase, "local");
  f.state.pending.current = false; f.state.phase = null; f.success();
  const next = f.state.notice; old.callback(); assert.equal(f.state.notice, next);
  f.advance(30_000); assert.equal(f.state.notice, null);
});

test("errors, Uncertain, unavailable status and active progress never auto-dismiss", () => {
  for (const notice of [{ kind: "error" }, { kind: "error", uncertain: true },
    { kind: "error", statusUnavailable: true }, { kind: "success", uncertain: true },
    { kind: "success", statusUnavailable: true }]) {
    const f = harness(); f.success(); const old = f.timers[0];
    f.state.notice = notice;
    old.callback(); assert.equal(f.state.notice, notice, "identity guard protects replacement before cleanup");
    f.render(); old.callback(); f.advance(60_000);
    assert.equal(f.state.notice, notice); assert.equal(f.timers.length, 1);
  }
  const f = harness(); f.state.phase = "local"; f.success();
  f.advance(60_000); assert.equal(f.timers.length, 0); assert.equal(f.state.phase, "local");
  f.state.phase = null; f.render(); f.advance(29_999); assert.equal(f.state.notice.kind, "success");
  f.advance(1); assert.equal(f.state.notice, null);
});

test("unmount cancels expiration and queued callbacks cannot update state", () => {
  const f = harness(); f.success(); const notice = f.state.notice;
  f.unmount(); assert.equal(f.timers[0].cancelled, true);
  f.timers[0].callback(); f.advance(30_000); assert.equal(f.state.notice, notice);
});

test("immediate and monitored Save successes share the notification effect", () => {
  assert.match(app, /operation\.phase === "completed"[\s\S]*?setConfigSaveNotice\(\{ kind: "success" \}\)/);
  assert.match(app, /response\.status === "accepted"[\s\S]*?else \{[\s\S]*?setConfigSaveNotice\(\{ kind: "success" \}\)/);
  assert.match(source, /\[configSaveNotice, configSavePhase\]/);
});
