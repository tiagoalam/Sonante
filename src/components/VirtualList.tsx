import React, {
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { isVirtualGridMeasurementReady } from "../utils/virtualGrid";
import { virtualListIndexOffset, virtualListMetrics } from "../utils/virtualList";

const OVERSCAN_ROWS = 5;

export interface VirtualListHandle {
  scrollToIndex: (index: number) => void;
}

export interface VirtualListProps<T> {
  items: readonly T[];
  rowHeight: number;
  getItemKey: (item: T) => React.Key;
  renderItem: (item: T) => React.ReactNode;
  scrollContainer: HTMLElement | null;
  resetKey?: string;
}

interface ViewportMeasurement {
  width: number;
  height: number;
  scrollOffset: number;
}

function VirtualListComponent<T>({
  items,
  rowHeight,
  getItemKey,
  renderItem,
  scrollContainer,
  resetKey,
}: VirtualListProps<T>, ref: React.ForwardedRef<VirtualListHandle>) {
  const listRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<number | null>(null);
  const pendingIndexRef = useRef<number | null>(null);
  const previousResetKey = useRef(resetKey);
  const [viewport, setViewport] = useState<ViewportMeasurement>({
    width: 0,
    height: 0,
    scrollOffset: 0,
  });

  const measure = useCallback((list: HTMLElement, container: HTMLElement) => {
    const listRect = list.getBoundingClientRect();
    const scrollRect = container.getBoundingClientRect();
    const next = {
      width: list.clientWidth,
      height: container.clientHeight,
      scrollOffset: Math.max(0, scrollRect.top - listRect.top),
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
    const list = listRef.current;
    if (!list || !scrollContainer) return;
    measure(list, scrollContainer);
    const scheduleMeasure = () => {
      if (frameRef.current !== null) return;
      frameRef.current = requestAnimationFrame(() => {
        frameRef.current = null;
        measure(list, scrollContainer);
      });
    };
    const observer = new ResizeObserver(scheduleMeasure);
    observer.observe(list);
    observer.observe(scrollContainer);
    scrollContainer.addEventListener("scroll", scheduleMeasure, { passive: true });
    return () => {
      scrollContainer.removeEventListener("scroll", scheduleMeasure);
      observer.disconnect();
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    };
  }, [measure, scrollContainer]);

  useLayoutEffect(() => {
    if (previousResetKey.current === resetKey) return;
    const list = listRef.current;
    if (!list || !scrollContainer) return;
    previousResetKey.current = resetKey;
    const listTop = list.getBoundingClientRect().top;
    const containerTop = scrollContainer.getBoundingClientRect().top;
    scrollContainer.scrollTop = Math.max(0, scrollContainer.scrollTop + listTop - containerTop);
    measure(list, scrollContainer);
  }, [measure, resetKey, scrollContainer]);

  const ready = isVirtualGridMeasurementReady(viewport.width, viewport.height);
  const metrics = virtualListMetrics({
    itemCount: items.length,
    rowHeight,
    scrollOffset: viewport.scrollOffset,
    viewportWidth: viewport.width,
    viewportHeight: viewport.height,
    overscanRows: OVERSCAN_ROWS,
  });

  const scrollToIndex = useCallback((index: number) => {
    const list = listRef.current;
    const itemOffset = virtualListIndexOffset(index, items.length, rowHeight);
    if (!ready || !list || !scrollContainer || itemOffset === null) {
      pendingIndexRef.current = Number.isFinite(index) && items.length > 0 ? index : null;
      return;
    }
    pendingIndexRef.current = null;
    const listTop = list.getBoundingClientRect().top;
    const containerTop = scrollContainer.getBoundingClientRect().top;
    const listOffset = scrollContainer.scrollTop + listTop - containerTop;
    scrollContainer.scrollTop = Math.max(0, listOffset + itemOffset);
    measure(list, scrollContainer);
  }, [items.length, measure, ready, rowHeight, scrollContainer]);

  useImperativeHandle(ref, () => ({ scrollToIndex }), [scrollToIndex]);

  useLayoutEffect(() => {
    if (!ready || pendingIndexRef.current === null) return;
    const index = pendingIndexRef.current;
    pendingIndexRef.current = null;
    scrollToIndex(index);
  }, [ready, scrollToIndex]);

  if (!ready) return <div ref={listRef} className="relative w-full" />;

  return (
    <div ref={listRef} className="relative w-full" style={{ height: metrics.totalHeight }}>
      {items.slice(metrics.startIndex, metrics.endIndex).map((item, offset) => {
        const index = metrics.startIndex + offset;
        return (
          <div
            key={getItemKey(item)}
            className="absolute left-0 w-full"
            style={{ top: index * rowHeight, height: rowHeight }}
          >
            {renderItem(item)}
          </div>
        );
      })}
    </div>
  );
}

export const VirtualList = React.forwardRef(VirtualListComponent) as <T>(
  props: VirtualListProps<T> & React.RefAttributes<VirtualListHandle>,
) => React.ReactElement;
