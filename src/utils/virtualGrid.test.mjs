import assert from "node:assert/strict";
import test from "node:test";
import {
  gridColumnCount,
  gridRowCount,
  isVirtualGridMeasurementReady,
  visibleRowRange,
  virtualGridMetrics,
} from "./virtualGrid.ts";

const layout = (overrides = {}) => virtualGridMetrics({
  itemCount: 100,
  containerWidth: 946,
  minimumColumnWidth: 170,
  gap: 24,
  cardExtraHeight: 64,
  scrollOffset: 0,
  viewportHeight: 700,
  overscanRows: 3,
  ...overrides,
});

test("handles zero and one item", () => {
  assert.equal(layout({ itemCount: 0 }).rowCount, 0);
  assert.equal(layout({ itemCount: 0 }).endRow, 0);
  assert.equal(layout({ itemCount: 1 }).rowCount, 1);
});

test("zero width produces no renderable geometry", () => {
  const metrics = layout({ containerWidth: 0 });
  assert.equal(isVirtualGridMeasurementReady(0, 700), false);
  assert.equal(isVirtualGridMeasurementReady(1_200, 0), false);
  assert.deepEqual(
    {
      columnCount: metrics.columnCount,
      cardWidth: metrics.cardWidth,
      cardHeight: metrics.cardHeight,
      totalHeight: metrics.totalHeight,
    },
    { columnCount: 0, cardWidth: 0, cardHeight: 0, totalHeight: 0 },
  );
  assert.deepEqual([metrics.startRow, metrics.endRow], [0, 0]);
});

test("a zero-width measurement becomes valid when the container is measured", () => {
  const unmeasured = layout({ containerWidth: 0 });
  const measured = layout({ containerWidth: 1_200 });
  assert.equal(unmeasured.endRow, 0);
  assert.equal(isVirtualGridMeasurementReady(1_200, 700), true);
  assert.equal(measured.columnCount, 6);
  assert.equal(measured.cardWidth, 180);
  assert.equal(measured.cardHeight, 244);
});

test("equal measurements on separate mounts produce identical geometry", () => {
  const firstMount = layout({ containerWidth: 1_200 });
  const secondMountStartsEmpty = layout({ containerWidth: 0 });
  const secondMount = layout({ containerWidth: 1_200 });
  assert.equal(secondMountStartsEmpty.endRow, 0);
  assert.deepEqual(secondMount, firstMount);
});

test("counts partial and multiple rows", () => {
  assert.equal(gridRowCount(3, 5), 1);
  assert.equal(gridRowCount(11, 5), 3);
});

test("calculates ranges at the start, middle, and end", () => {
  const start = visibleRowRange(100, 200, 0, 600, 2);
  assert.deepEqual([start.startRow, start.endRow], [0, 5]);

  const middle = visibleRowRange(100, 200, 8_000, 600, 2);
  assert.deepEqual([middle.startRow, middle.endRow], [38, 45]);

  const end = visibleRowRange(100, 200, Number.MAX_SAFE_INTEGER, 600, 2);
  assert.deepEqual([end.startRow, end.endRow], [95, 100]);
});

test("overscan remains inside row bounds", () => {
  const start = visibleRowRange(4, 200, 0, 200, 99);
  const end = visibleRowRange(4, 200, 10_000, 200, 99);
  assert.equal(start.startRow, 0);
  assert.equal(end.endRow, 4);
});

test("resize recalculates five columns as three", () => {
  assert.equal(gridColumnCount(946, 170, 24), 5);
  assert.equal(gridColumnCount(558, 170, 24), 3);
  assert.equal(gridRowCount(12, 5), 3);
  assert.equal(gridRowCount(12, 3), 4);
});

test("ten thousand items still produce a small mounted range", () => {
  const metrics = layout({ itemCount: 10_000 });
  const mountedItems = (metrics.endRow - metrics.startRow) * metrics.columnCount;
  assert.equal(metrics.rowCount, 2_000);
  assert.ok(mountedItems <= 35, `expected at most 35 slots, got ${mountedItems}`);
});

test("scroll beyond content is clamped", () => {
  const metrics = layout({ itemCount: 20, scrollOffset: 1e9 });
  assert.equal(metrics.clampedScrollOffset, Math.max(0, metrics.rowCount * metrics.rowStride - 700));
  assert.equal(metrics.endRow, metrics.rowCount);
});

test("a small filtered result has only its result rows", () => {
  const metrics = layout({ itemCount: 4 });
  assert.equal(metrics.rowCount, 1);
  assert.deepEqual([metrics.startRow, metrics.endRow], [0, 1]);
});

test("invalid dimensions never produce NaN or an infinite range", () => {
  const metrics = layout({
    itemCount: Number.NaN,
    containerWidth: Number.NaN,
    minimumColumnWidth: 0,
    gap: Number.POSITIVE_INFINITY,
    cardExtraHeight: Number.NEGATIVE_INFINITY,
    scrollOffset: Number.NaN,
    viewportHeight: Number.POSITIVE_INFINITY,
    overscanRows: Number.NaN,
  });
  for (const value of Object.values(metrics)) assert.ok(Number.isFinite(value));
  assert.equal(metrics.rowCount, 0);
  assert.equal(metrics.startRow, 0);
  assert.equal(metrics.endRow, 0);
});

test("start offset corresponds to the first rendered row", () => {
  const metrics = layout({ scrollOffset: 5_000 });
  assert.equal(metrics.startOffset, metrics.startRow * metrics.rowStride);
});
