export interface VisibleRowRange {
  startRow: number;
  endRow: number;
  visibleStartRow: number;
  visibleEndRow: number;
  clampedScrollOffset: number;
}

export interface VirtualGridMetrics extends VisibleRowRange {
  columnCount: number;
  rowCount: number;
  cardWidth: number;
  cardHeight: number;
  rowStride: number;
  totalHeight: number;
  startOffset: number;
}

export interface VirtualGridIndexPosition {
  index: number;
  rowIndex: number;
  rowOffset: number;
}

export function isVirtualGridMeasurementReady(
  width: number,
  height: number,
): boolean {
  return Number.isFinite(width) && width > 0
    && Number.isFinite(height) && height > 0;
}

function finiteOr(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

function nonNegativeInteger(value: number): number {
  return Math.max(0, Math.floor(finiteOr(value, 0)));
}

export function gridColumnCount(
  containerWidth: number,
  minimumColumnWidth: number,
  gap: number,
): number {
  const width = Math.max(0, finiteOr(containerWidth, 0));
  const minimum = finiteOr(minimumColumnWidth, 1);
  const safeMinimum = minimum > 0 ? minimum : 1;
  const safeGap = Math.max(0, finiteOr(gap, 0));
  return Math.max(1, Math.floor((width + safeGap) / (safeMinimum + safeGap)));
}

export function gridRowCount(itemCount: number, columnCount: number): number {
  const items = nonNegativeInteger(itemCount);
  const columns = Math.max(1, nonNegativeInteger(columnCount));
  return Math.ceil(items / columns);
}

export function virtualGridIndexPosition(
  requestedIndex: number,
  itemCount: number,
  columnCount: number,
  rowStride: number,
): VirtualGridIndexPosition | null {
  const items = nonNegativeInteger(itemCount);
  const columns = nonNegativeInteger(columnCount);
  const stride = finiteOr(rowStride, 0);
  if (!Number.isFinite(requestedIndex) || items === 0 || columns === 0 || stride <= 0) {
    return null;
  }
  const index = Math.min(items - 1, Math.max(0, Math.floor(requestedIndex)));
  const rowIndex = Math.floor(index / columns);
  return { index, rowIndex, rowOffset: rowIndex * stride };
}

export function visibleRowRange(
  rowCount: number,
  rowStride: number,
  scrollOffset: number,
  viewportHeight: number,
  overscanRows: number,
): VisibleRowRange {
  const rows = nonNegativeInteger(rowCount);
  const stride = finiteOr(rowStride, 1) > 0 ? finiteOr(rowStride, 1) : 1;
  const viewport = Math.max(0, finiteOr(viewportHeight, 0));
  const overscan = nonNegativeInteger(overscanRows);
  const maximumOffset = Math.max(0, rows * stride - viewport);
  const offset = Math.min(
    maximumOffset,
    Math.max(0, finiteOr(scrollOffset, 0)),
  );

  if (rows === 0) {
    return {
      startRow: 0,
      endRow: 0,
      visibleStartRow: 0,
      visibleEndRow: 0,
      clampedScrollOffset: 0,
    };
  }

  const visibleStartRow = Math.min(rows - 1, Math.floor(offset / stride));
  const visibleEndRow = Math.min(
    rows,
    Math.max(visibleStartRow + 1, Math.ceil((offset + viewport) / stride)),
  );

  return {
    startRow: Math.max(0, visibleStartRow - overscan),
    endRow: Math.min(rows, visibleEndRow + overscan),
    visibleStartRow,
    visibleEndRow,
    clampedScrollOffset: offset,
  };
}

export function virtualGridMetrics({
  itemCount,
  containerWidth,
  minimumColumnWidth,
  gap,
  cardExtraHeight,
  scrollOffset,
  viewportHeight,
  overscanRows,
}: {
  itemCount: number;
  containerWidth: number;
  minimumColumnWidth: number;
  gap: number;
  cardExtraHeight: number;
  scrollOffset: number;
  viewportHeight: number;
  overscanRows: number;
}): VirtualGridMetrics {
  if (!isVirtualGridMeasurementReady(containerWidth, viewportHeight)) {
    return {
      columnCount: 0,
      rowCount: 0,
      cardWidth: 0,
      cardHeight: 0,
      rowStride: 0,
      totalHeight: 0,
      startOffset: 0,
      startRow: 0,
      endRow: 0,
      visibleStartRow: 0,
      visibleEndRow: 0,
      clampedScrollOffset: 0,
    };
  }

  const width = Math.max(0, finiteOr(containerWidth, 0));
  const safeGap = Math.max(0, finiteOr(gap, 0));
  const columnCount = gridColumnCount(width, minimumColumnWidth, safeGap);
  const rowCount = gridRowCount(itemCount, columnCount);
  const cardWidth = Math.max(0, (width - safeGap * (columnCount - 1)) / columnCount);
  const cardHeight = cardWidth + Math.max(0, finiteOr(cardExtraHeight, 0));
  const rowStride = Math.max(1, cardHeight + safeGap);
  const totalHeight = rowCount === 0
    ? 0
    : rowCount * cardHeight + (rowCount - 1) * safeGap;
  const range = visibleRowRange(
    rowCount,
    rowStride,
    scrollOffset,
    viewportHeight,
    overscanRows,
  );

  return {
    ...range,
    columnCount,
    rowCount,
    cardWidth,
    cardHeight,
    rowStride,
    totalHeight,
    startOffset: range.startRow * rowStride,
  };
}
