import React, { useLayoutEffect, useRef, useState } from "react";
import { VirtualAlbumGrid } from "./VirtualAlbumGrid";
import type { ExploreEntry } from "../utils/explore";

// A separate native viewport keeps genre position independent of all shelves.
// Restore only after the existing virtual grid has published its full height.
export const ExploreGenreSlice: React.FC<{ id: string; label: string; entries: readonly ExploreEntry[];
  active: boolean; positions: Record<string, number>; renderItem: (entry: ExploreEntry) => React.ReactNode }> = ({ id, label, entries, active, positions, renderItem }) => {
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null);
  const [content, setContent] = useState<HTMLDivElement | null>(null);
  const restoring = useRef(true);
  useLayoutEffect(() => {
    restoring.current = true;
    if (!active || !viewport || !content) return;
    const restore = () => {
      if (!restoring.current || viewport.clientHeight <= 0 || content.scrollHeight <= 0) return;
      viewport.scrollTop = positions[id] ?? 0;
      restoring.current = false;
    };
    const save = () => {
      if (!restoring.current && viewport.clientHeight > 0) positions[id] = viewport.scrollTop;
    };
    const observer = new ResizeObserver(restore); observer.observe(content); observer.observe(viewport);
    viewport.addEventListener("scroll", save, { passive: true }); restore();
    return () => { observer.disconnect(); viewport.removeEventListener("scroll", save); restoring.current = true; };
  }, [active, viewport, content, id, positions]);
  return <div ref={setViewport} role="region" aria-label={label} tabIndex={0}
    className="mt-3 min-h-0 flex-1 overflow-y-auto py-1 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
    <div ref={setContent}><VirtualAlbumGrid items={entries} getItemKey={entry => entry.key}
      scrollContainer={viewport} resetKey={id} renderItem={renderItem} /></div>
  </div>;
};
