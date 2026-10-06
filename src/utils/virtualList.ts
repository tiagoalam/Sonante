import { isVirtualGridMeasurementReady, visibleRowRange } from "./virtualGrid.ts";

export interface VirtualListMetrics {
  startIndex: number;
  endIndex: number;
  totalHeight: number;
  clampedScrollOffset: number;
}

export function virtualListMetrics({
  itemCount,
  rowHeight,
  scrollOffset,
  viewportWidth,
  viewportHeight,
  overscanRows,
}: {
  itemCount: number;
  rowHeight: number;
  scrollOffset: number;
  viewportWidth: number;
  viewportHeight: number;
  overscanRows: number;
}): VirtualListMetrics {
  if (!isVirtualGridMeasurementReady(viewportWidth, viewportHeight)
    || !Number.isFinite(rowHeight) || rowHeight <= 0) {
    return { startIndex: 0, endIndex: 0, totalHeight: 0, clampedScrollOffset: 0 };
  }
  const count = Math.max(0, Math.floor(Number.isFinite(itemCount) ? itemCount : 0));
  const range = visibleRowRange(count, rowHeight, scrollOffset, viewportHeight, overscanRows);
  return {
    startIndex: range.startRow,
    endIndex: range.endRow,
    totalHeight: count * rowHeight,
    clampedScrollOffset: range.clampedScrollOffset,
  };
}

export function virtualListIndexOffset(
  requestedIndex: number,
  itemCount: number,
  rowHeight: number,
): number | null {
  if (!Number.isFinite(requestedIndex) || !Number.isFinite(rowHeight) || rowHeight <= 0) return null;
  const count = Math.max(0, Math.floor(Number.isFinite(itemCount) ? itemCount : 0));
  if (count === 0) return null;
  const index = Math.min(count - 1, Math.max(0, Math.floor(requestedIndex)));
  return index * rowHeight;
}
