import assert from "node:assert/strict";
import test from "node:test";
import { virtualListIndexOffset, virtualListMetrics } from "./virtualList.ts";

const metrics = (overrides = {}) => virtualListMetrics({
  itemCount: 100,
  rowHeight: 56,
  scrollOffset: 0,
  viewportWidth: 800,
  viewportHeight: 560,
  overscanRows: 5,
  ...overrides,
});

test("handles zero and one row", () => {
  assert.deepEqual(metrics({ itemCount: 0 }), { startIndex: 0, endIndex: 0, totalHeight: 0, clampedScrollOffset: 0 });
  assert.deepEqual(metrics({ itemCount: 1 }), { startIndex: 0, endIndex: 1, totalHeight: 56, clampedScrollOffset: 0 });
});

test("returns bounded ranges at the start, middle, and end with overscan", () => {
  assert.deepEqual([metrics().startIndex, metrics().endIndex], [0, 15]);
  const middle = metrics({ scrollOffset: 2_800 });
  assert.deepEqual([middle.startIndex, middle.endIndex], [45, 65]);
  const end = metrics({ scrollOffset: 99_999 });
  assert.deepEqual([end.startIndex, end.endIndex], [85, 100]);
  assert.equal(end.clampedScrollOffset, 5_040);
});

test("five thousand artists still produce a viewport-sized range", () => {
  const result = metrics({ itemCount: 5_000, scrollOffset: 100_000 });
  assert.ok(result.endIndex - result.startIndex <= 21);
  assert.equal(result.totalHeight, 280_000);
});

test("invalid mount measurements are safe and a later resize recomputes the range", () => {
  assert.equal(metrics({ viewportWidth: 0 }).endIndex, 0);
  assert.equal(metrics({ viewportHeight: 0 }).endIndex, 0);
  assert.equal(metrics({ rowHeight: 0 }).endIndex, 0);
  assert.ok(metrics({ viewportHeight: 280 }).endIndex < metrics({ viewportHeight: 840 }).endIndex);
  assert.deepEqual(metrics(), metrics());
});

test("index offsets clamp and reject invalid geometry", () => {
  assert.equal(virtualListIndexOffset(-5, 10, 56), 0);
  assert.equal(virtualListIndexOffset(99, 10, 56), 504);
  assert.equal(virtualListIndexOffset(4, 0, 56), null);
  assert.equal(virtualListIndexOffset(4, 10, 0), null);
  assert.equal(virtualListIndexOffset(Number.NaN, 10, 56), null);
});
