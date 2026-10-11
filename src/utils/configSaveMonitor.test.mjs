import test from "node:test";
import assert from "node:assert/strict";
import { monitorConfigSave, configSavePending } from "./configSaveMonitor.ts";

const operation = (phase, id = "1") => ({ id, phase, error: phase === "failed" ? "fixture failure" : null });
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };

function fixture() {
  let snapshot = null;
  const changes = [], errors = [], scheduled = new Map();
  let nextTimer = 0;
  const monitor = monitorConfigSave(
    async () => { if (snapshot instanceof Error) throw snapshot; return snapshot; },
    (state) => changes.push(state), (error) => errors.push(error),
    (callback) => { scheduled.set(++nextTimer, callback); return nextTimer; },
    (timer) => scheduled.delete(timer),
  );
  return { monitor, changes, errors, scheduled,
    async poll(state) {
      snapshot = state;
      const [timer, callback] = scheduled.entries().next().value;
      scheduled.delete(timer); callback(); await flush();
    },
  };
}

test("accepted, preparing, indexing and validating never declare success", async () => {
  const f = fixture(); await flush();
  f.monitor.beginAcceptance();
  f.monitor.accepted(operation("preparing"));
  await f.poll(operation("indexing"));
  await f.poll(operation("indexing"));
  await f.poll(operation("validating"));
  assert.ok(f.changes.every(configSavePending));
  assert.ok(f.monitor.isPending());
  await f.poll(operation("completed"));
  await f.poll(operation("completed"));
  assert.equal(f.changes.filter((s) => s.phase === "completed").length, 1);
  assert.equal(f.monitor.isPending(), false);
  f.monitor.dispose();
});

test("late acceptance and old snapshots cannot overwrite completion or a newer operation", async () => {
  const f = fixture(); await flush();
  f.monitor.accepted(operation("preparing"));
  await f.poll(operation("completed"));
  f.monitor.accepted(operation("preparing"));
  assert.equal(f.changes.at(-1).phase, "completed");
  f.monitor.beginAcceptance();
  await f.poll(operation("completed"));
  f.monitor.accepted(operation("preparing", "2"));
  await f.poll(operation("completed"));
  assert.equal(f.changes.at(-1).id, "2");
  assert.equal(f.changes.at(-1).phase, "preparing");
  await f.poll(operation("validating", "2"));
  await f.poll(operation("indexing", "2"));
  assert.equal(f.changes.at(-1).phase, "validating");
  f.monitor.dispose();
});

test("failed acceptance cannot be reported as success by a historical operation", async () => {
  const f = fixture(); await flush();
  f.monitor.beginAcceptance();
  await f.poll(operation("completed"));
  f.monitor.acceptanceFailed();
  await f.poll(operation("completed"));
  assert.deepEqual(f.changes, []);
  f.monitor.dispose();
});

test("renderer reload observes a pending task and failed or uncertain outcomes", async () => {
  for (const terminal of ["failed", "uncertain"]) {
    const f = fixture(); await flush();
    await f.poll(operation("indexing"));
    assert.ok(f.monitor.isPending());
    await f.poll(operation(terminal));
    assert.equal(f.changes.at(-1).phase, terminal);
    assert.ok(!f.changes.some((s) => s.phase === "completed"));
    f.monitor.dispose();
  }
});

test("renderer reload can observe completion even if the task finishes before its first query", async () => {
  const f = fixture(); await flush();
  await f.poll(operation("completed"));
  assert.equal(f.changes.at(-1).phase, "completed");
  f.monitor.dispose();
});

test("observer failure keeps the operation pending and retries; disposal cancels polling", async () => {
  const f = fixture(); await flush();
  f.monitor.accepted(operation("indexing"));
  await f.poll(new Error("read failed"));
  assert.equal(f.errors.length, 1);
  assert.ok(f.monitor.isPending());
  await f.poll(operation("indexing"));
  assert.ok(f.changes.every(configSavePending));
  f.monitor.dispose();
  assert.equal(f.scheduled.size, 0);
});

test("disposing an in-flight read cannot publish or restart its timer", async () => {
  let resolve;
  const changes = [];
  const monitor = monitorConfigSave(() => new Promise((r) => { resolve = r; }),
    (s) => changes.push(s), assert.fail, () => assert.fail("unexpected timer"));
  monitor.dispose();
  resolve(operation("completed")); await flush();
  assert.deepEqual(changes, []);
});
