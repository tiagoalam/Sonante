import React, {
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  isVirtualGridMeasurementReady,
  virtualGridIndexPosition,
  virtualGridMetrics,
} from "../utils/virtualGrid";

const MINIMUM_CARD_WIDTH = 170;
const GRID_GAP = 24;
const CARD_TEXT_HEIGHT = 64;
const OVERSCAN_ROWS = 3;

export interface VirtualAlbumGridProps<T> {
  items: readonly T[];
  getItemKey: (item: T) => React.Key;
  renderItem: (item: T) => React.ReactNode;
  scrollContainer: HTMLElement | null;
  resetKey?: string;
}

export interface VirtualAlbumGridHandle {
  scrollToIndex: (index: number) => void;
}

interface ViewportMeasurement {
  width: number;
  height: number;
  scrollOffset: number;
}

function VirtualAlbumGridComponent<T>({
  items,
  getItemKey,
  renderItem,
  scrollContainer,
  resetKey,
}: VirtualAlbumGridProps<T>, ref: React.ForwardedRef<VirtualAlbumGridHandle>) {
  const gridRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<number | null>(null);
  const pendingIndexRef = useRef<number | null>(null);
  const previousResetKey = useRef(resetKey);
  const [viewport, setViewport] = useState<ViewportMeasurement>({
    width: 0,
    height: 0,
    scrollOffset: 0,
  });

  const measure = useCallback((grid: HTMLElement, container: HTMLElement) => {
    const gridRect = grid.getBoundingClientRect();
    const scrollRect = container.getBoundingClientRect();
    const next = {
      width: grid.clientWidth,
      height: container.clientHeight,
      scrollOffset: Math.max(0, scrollRect.top - gridRect.top),
    };
    if (!isVirtualGridMeasurementReady(next.width, next.height)) return;

    setViewport((current) => (
      current.width === next.width
      && current.height === next.height
      && current.scrollOffset === next.scrollOffset
        ? current
        : next
    ));
  }, []);

  useLayoutEffect(() => {
    const grid = gridRef.current;
    if (!grid || !scrollContainer) return;

    measure(grid, scrollContainer);
    const scheduleMeasure = () => {
      if (frameRef.current !== null) return;
      frameRef.current = requestAnimationFrame(() => {
        frameRef.current = null;
        measure(grid, scrollContainer);
      });
    };
    const resizeObserver = new ResizeObserver(scheduleMeasure);
    resizeObserver.observe(grid);
    resizeObserver.observe(scrollContainer);
    scrollContainer.addEventListener("scroll", scheduleMeasure, { passive: true });

    return () => {
      scrollContainer.removeEventListener("scroll", scheduleMeasure);
      resizeObserver.disconnect();
      if (frameRef.current !== null) {
        cancelAnimationFrame(frameRef.current);
        frameRef.current = null;
      }
    };
  }, [measure, scrollContainer]);

  useLayoutEffect(() => {
    if (previousResetKey.current === resetKey) return;

    const grid = gridRef.current;
    if (!grid || !scrollContainer) return;
    previousResetKey.current = resetKey;
    const gridTop = grid.getBoundingClientRect().top;
    const scrollTop = scrollContainer.getBoundingClientRect().top;
    scrollContainer.scrollTop = Math.max(0, scrollContainer.scrollTop + gridTop - scrollTop);
    measure(grid, scrollContainer);
  }, [measure, resetKey, scrollContainer]);

  const measurementReady = isVirtualGridMeasurementReady(viewport.width, viewport.height);
  const metrics = virtualGridMetrics({
    itemCount: items.length,
    containerWidth: viewport.width,
    minimumColumnWidth: MINIMUM_CARD_WIDTH,
    gap: GRID_GAP,
    cardExtraHeight: CARD_TEXT_HEIGHT,
    scrollOffset: viewport.scrollOffset,
    viewportHeight: viewport.height,
    overscanRows: OVERSCAN_ROWS,
  });

  const scrollToIndex = useCallback((index: number) => {
    const grid = gridRef.current;
    const position = virtualGridIndexPosition(
      index,
      items.length,
      metrics.columnCount,
      metrics.rowStride,
    );
    if (!measurementReady || !grid || !scrollContainer || !position) {
      pendingIndexRef.current = Number.isFinite(index) && items.length > 0 ? index : null;
      return;
    }

    pendingIndexRef.current = null;
    const gridTop = grid.getBoundingClientRect().top;
    const scrollTop = scrollContainer.getBoundingClientRect().top;
    const gridOffset = scrollContainer.scrollTop + gridTop - scrollTop;
    scrollContainer.scrollTop = Math.max(0, gridOffset + position.rowOffset);
    measure(grid, scrollContainer);
  }, [items.length, measure, measurementReady, metrics.columnCount, metrics.rowStride, scrollContainer]);

  useImperativeHandle(ref, () => ({ scrollToIndex }), [scrollToIndex]);

  useLayoutEffect(() => {
    if (!measurementReady || pendingIndexRef.current === null) return;
    const pendingIndex = pendingIndexRef.current;
    pendingIndexRef.current = null;
    scrollToIndex(pendingIndex);
  }, [measurementReady, scrollToIndex]);

  if (!measurementReady) {
    return <div ref={gridRef} className="relative w-full" />;
  }

  const rows = [];
  for (let rowIndex = metrics.startRow; rowIndex < metrics.endRow; rowIndex += 1) {
    const firstItem = rowIndex * metrics.columnCount;
    const rowItems = items.slice(firstItem, firstItem + metrics.columnCount);
    rows.push(
      <div
        key={rowIndex}
        className="absolute left-0 grid w-full gap-6"
        style={{
          top: rowIndex * metrics.rowStride,
          height: metrics.cardHeight,
          gridTemplateColumns: `repeat(${metrics.columnCount}, minmax(0, 1fr))`,
        }}
      >
        {rowItems.map((item) => (
          <React.Fragment key={getItemKey(item)}>{renderItem(item)}</React.Fragment>
        ))}
      </div>,
    );
  }

  return (
    <div
      ref={gridRef}
      className="relative w-full"
      style={{ height: metrics.totalHeight }}
    >
      {rows}
    </div>
  );
}

export const VirtualAlbumGrid = React.forwardRef(VirtualAlbumGridComponent) as <T>(
  props: VirtualAlbumGridProps<T> & React.RefAttributes<VirtualAlbumGridHandle>,
) => React.ReactElement;
