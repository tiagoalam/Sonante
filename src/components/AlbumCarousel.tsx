import React, { useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { VirtualAlbumGridHandle } from "./VirtualAlbumGrid";
import { carouselWindow, carouselKeyDirection, carouselSwipeDirection, carouselCoverSize } from "../utils/albumCarousel.mjs";
import "./discotheque.css";

type Props<T> = { items: readonly T[]; getItemKey: (item: T) => React.Key; renderItem: (item: T, context?: { focused: boolean }) => React.ReactNode; resetKey?: string; onReachEnd?: () => void; sourceLabel?: string; scrollContainer?: HTMLElement | null; cardExtraHeight?: number };
function AlbumCarouselComponent<T>({items, getItemKey, renderItem, resetKey, onReachEnd, sourceLabel, scrollContainer, cardExtraHeight = 30}: Props<T>, ref: React.ForwardedRef<VirtualAlbumGridHandle>) {
  const { t } = useTranslation();
  const [index, setIndex] = useState(0);
  const root = useRef<HTMLElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const [dimensions, setDimensions] = useState({ height: 0, cover: 0 });
  const touch = useRef<number | null>(null);
  const suppressClickUntil = useRef(0);
  const { selected, start, end } = carouselWindow(items.length, index);
  useEffect(() => { setIndex(0); }, [resetKey]);
  useImperativeHandle(ref, () => ({ scrollToIndex: i => setIndex(Math.max(0, Math.min(items.length - 1, i))) }), [items.length]);
  useLayoutEffect(() => {
    if (!scrollContainer || !root.current || !stage.current) return;
    const update = () => {
      if (!root.current || !stage.current) return;
      const offset = Math.max(0, root.current.getBoundingClientRect().top - scrollContainer.getBoundingClientRect().top);
      const padding = parseFloat(getComputedStyle(scrollContainer).paddingBottom) || 0;
      const height = Math.max(0, Math.floor(scrollContainer.clientHeight - offset - padding));
      const cover = carouselCoverSize(stage.current.clientWidth, stage.current.clientHeight, cardExtraHeight);
      setDimensions(previous => previous.height === height && previous.cover === cover ? previous : { height, cover });
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(scrollContainer);
    observer.observe(stage.current);
    return () => observer.disconnect();
  }, [scrollContainer, cardExtraHeight, items.length > 0]);
  const move = (next: number) => {
    if (next >= items.length - 2) onReachEnd?.();
    setIndex(Math.max(0, Math.min(items.length - 1, next)));
  };
  if (!items.length) return <div className="py-10 text-center text-sm" role="status">{t("albumViews.emptyCarousel")}</div>;
  const item = items[selected] as { title?: unknown; artist?: unknown; year?: unknown; track_count?: unknown; song_count?: unknown; source?: unknown; source_label?: unknown };
  const title = typeof item.title === "string" ? item.title : "";
  const artist = typeof item.artist === "string" ? item.artist : "";
  const year = typeof item.year === "number" || typeof item.year === "string" ? String(item.year) : "";
  const count = typeof item.track_count === "number" ? item.track_count : typeof item.song_count === "number" ? item.song_count : null;
  const memberSource = item.source === "local" || item.source === "plex" || item.source === "subsonic" ? t(`collections.sources.${item.source}`) : null;
  const origin = typeof item.source_label === "string" && item.source_label ? item.source_label : memberSource || sourceLabel;
  return <section ref={root} className="album-carousel" aria-label={t("albumViews.carousel")} tabIndex={0} style={dimensions.height ? { height: dimensions.height } : undefined}
    onKeyDown={event => {
      const target = event.target as HTMLElement;
      if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || event.nativeEvent?.isComposing ||
        (typeof target.closest === "function" && target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="slider"], [role="menu"], [role="dialog"]'))) return;
      if (event.key === "Enter" && event.target === event.currentTarget) {
        const open = stage.current?.querySelector<HTMLElement>('[data-position="0"] [data-album-open]');
        if (open && !open.matches(':disabled') && open.getAttribute('aria-disabled') !== 'true') {
          event.preventDefault();
          open.click();
        }
        return;
      }
      if (event.target !== event.currentTarget) return;
      const direction = carouselKeyDirection(event.key); if (direction) { event.preventDefault(); event.stopPropagation(); move(selected + direction); }
    }}
    onClickCapture={event => { if (Date.now() < suppressClickUntil.current) { event.preventDefault(); event.stopPropagation(); } }}
    onTouchStart={event => { touch.current = event.touches[0]?.clientX ?? null; }}
    onTouchEnd={event => { const end = event.changedTouches[0]?.clientX; const direction = carouselSwipeDirection(touch.current, end); if (direction) { suppressClickUntil.current = Date.now() + 450; move(selected + direction); } touch.current = null; }}>
    <div className="album-carousel-spotlight" aria-live="polite">
      {origin && <span className="album-carousel-origin">{origin}</span>}
      {title && <h2 title={title}>{title}</h2>}
      {artist && <p className="album-carousel-artist">{artist}</p>}
      {(year || count != null) && <p className="album-carousel-metadata">{year}{year && count != null ? " · " : ""}{count != null ? t("albumViews.trackCount", { count }) : ""}</p>}
    </div>
    <div ref={stage} className="album-carousel-stage" style={dimensions.cover ? { "--carousel-cover-size": `${dimensions.cover}px` } as React.CSSProperties : undefined}>
      {items.slice(start, end).map((item, offset) => {
        const itemIndex = start + offset;
        const delta = itemIndex - selected;
        return <div className="album-carousel-item" data-position={delta} key={getItemKey(item)} style={{ zIndex: 10 - Math.abs(delta) }}
          onClickCapture={event => { if (delta !== 0) { event.preventDefault(); event.stopPropagation(); move(itemIndex); } }}>
          <div aria-hidden={delta !== 0 ? true : undefined} inert={delta !== 0 ? true : undefined}>{renderItem(item, { focused: delta === 0 })}</div>
        </div>;
      })}
    </div>
    <div className="album-carousel-controls"><button type="button" aria-label={t("albumViews.previousAlbum")} disabled={selected === 0} onClick={() => move(selected - 1)}><ChevronLeft size={20} /></button><span aria-live="polite">{selected + 1} / {items.length}</span><button type="button" aria-label={t("albumViews.nextAlbum")} disabled={selected >= items.length - 1 && !onReachEnd} onClick={() => move(selected + 1)}><ChevronRight size={20} /></button></div>
  </section>;
}
export const AlbumCarousel = React.forwardRef(AlbumCarouselComponent) as <T>(props: Props<T> & React.RefAttributes<VirtualAlbumGridHandle>) => React.ReactElement;
