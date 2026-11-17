import test from "node:test";
import assert from "node:assert/strict";
import { createLibraryActivityVisibility, LIBRARY_ACTIVITY_SUCCESS_HIDE_MS } from "./libraryActivityLifecycle.ts";

function scheduler() {
  const tasks = new Map(); let next = 0;
  return {
    tasks,
    schedule(callback, delay) { const id = ++next; tasks.set(id, { callback, delay }); return id; },
    cancel(id) { tasks.delete(id); },
    fire(id) { const task = tasks.get(id); if (task) { tasks.delete(id); task.callback(); } },
  };
}

function visibilityFixture() {
  const clock = scheduler(), published = [];
  const visibility = createLibraryActivityVisibility(value => published.push(value), clock.schedule, clock.cancel);
  return { clock, published, visibility };
}

test("successful activity hides after 60 seconds even while Local is unmounted", () => {
  const { clock, published, visibility } = visibilityFixture();
  visibility.begin("op-1");
  visibility.update({ operationId: "op-1", phase: "completed" });
  const [[id, timer]] = clock.tasks;
  assert.equal(LIBRARY_ACTIVITY_SUCCESS_HIDE_MS, 60_000);
  assert.equal(timer.delay, LIBRARY_ACTIVITY_SUCCESS_HIDE_MS);
  assert.equal(visibility.isHidden("op-1"), false, "completion does not hide immediately");
  // The controller belongs to App, not LocalBrowserView; navigating away only
  // unmounts the consumer and leaves this deadline alive.
  clock.fire(id);
  assert.equal(visibility.isHidden("op-1"), true);
  assert.deepEqual(published, [null, "op-1"]);
});

test("manual dismissal persists across view remounts and a new operation reactivates the panel", () => {
  const { clock, published, visibility } = visibilityFixture();
  visibility.begin("op-1");
  visibility.update({ operationId: "op-1", phase: "failed" });
  assert.equal(clock.tasks.size, 0, "errors do not auto-hide");
  visibility.dismiss("op-1");
  assert.equal(visibility.isHidden("op-1"), true);
  // Re-rendering or remounting the Local view does not recreate this state.
  assert.equal(visibility.isHidden("op-1"), true);
  visibility.begin("op-2");
  assert.equal(visibility.isHidden("op-2"), false);
  assert.deepEqual(published, [null, "op-1", null]);
});

test("new operations cancel old timers and stale operation events cannot revive old results", () => {
  const { clock, visibility } = visibilityFixture();
  visibility.begin("op-1");
  visibility.update({ operationId: "op-1", phase: "completed" });
  const id = [...clock.tasks.keys()][0];
  visibility.begin("op-2");
  assert.equal(clock.tasks.size, 0);
  visibility.update({ operationId: "op-1", phase: "failed" });
  clock.fire(id);
  assert.equal(visibility.isHidden("op-1"), false);
  assert.equal(visibility.isHidden("op-2"), false);
  assert.equal(clock.tasks.size, 0);
});

test("duplicate completion events do not restart the success deadline; failures remain visible", () => {
  const { clock, visibility } = visibilityFixture();
  visibility.update({ operationId: "op-1", phase: "metadata" });
  assert.equal(clock.tasks.size, 0);
  visibility.update({ operationId: "op-1", phase: "completed" });
  const firstId = [...clock.tasks.keys()][0];
  visibility.update({ operationId: "op-1", phase: "completed" });
  assert.deepEqual([...clock.tasks.keys()], [firstId]);
  clock.fire(firstId);
  visibility.begin("op-2");
  visibility.update({ operationId: "op-2", phase: "failed" });
  assert.equal(clock.tasks.size, 0);
  assert.equal(visibility.isHidden("op-2"), false);
});

test("StrictMode effect replay re-arms one completion timer without reviving a dismissed result", () => {
  const { clock, visibility } = visibilityFixture();
  visibility.update({ operationId: "op-1", phase: "completed" });
  const firstId = [...clock.tasks.keys()][0];
  visibility.dispose();
  assert.equal(clock.tasks.size, 0);
  visibility.update({ operationId: "op-1", phase: "completed" });
  assert.equal(clock.tasks.size, 1);
  const replayId = [...clock.tasks.keys()][0];
  assert.notEqual(replayId, firstId);
  clock.fire(replayId);
  assert.equal(visibility.isHidden("op-1"), true);
  visibility.dispose();
  visibility.update({ operationId: "op-1", phase: "completed" });
  assert.equal(clock.tasks.size, 0);
  assert.equal(visibility.isHidden("op-1"), true);
});
