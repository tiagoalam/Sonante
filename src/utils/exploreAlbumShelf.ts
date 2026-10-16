export const ALBUM_SHELF_GAP = 24;
const MINIMUM_CARD_WIDTH = 170;
const MAXIMUM_CARD_WIDTH = 220;

// Width is the shelf's own content box, after sidebar and page padding.
export function albumShelfLayout(width: number, count: number) {
  const viewportWidth = Number.isFinite(width) ? Math.max(0, width) : 0;
  const columns = Math.max(1, Math.min(count || 1, Math.floor((viewportWidth + ALBUM_SHELF_GAP) / (MINIMUM_CARD_WIDTH + ALBUM_SHELF_GAP))));
  const cardWidth = viewportWidth > 0 ? Math.min(MAXIMUM_CARD_WIDTH, (viewportWidth - (columns - 1) * ALBUM_SHELF_GAP) / columns) : 190;
  const stride = cardWidth + ALBUM_SHELF_GAP;
  const visibleCount = Math.max(1, Math.min(count || 1, Math.floor((viewportWidth + ALBUM_SHELF_GAP) / stride)));
  return { viewportWidth, cardWidth, stride, visibleCount };
}

export function albumShelfEdges(width: number, scrollWidth: number, left: number) {
  const maximum = Math.max(0, scrollWidth - width);
  return { previous: maximum > 1 && left > 1, next: maximum > 1 && left < maximum - 1 };
}

export function albumShelfTarget(width: number, scrollWidth: number, left: number,
  layout: ReturnType<typeof albumShelfLayout>, direction: -1 | 1 | "first" | "last") {
  const maximum = Math.max(0, scrollWidth - width);
  if (direction === "first") return 0;
  if (direction === "last") return maximum;
  const index = Math.max(0, direction < 0 ? Math.ceil((left - 1) / layout.stride) : Math.floor((left + 1) / layout.stride));
  return Math.max(0, Math.min(maximum, (index + direction * layout.visibleCount) * layout.stride));
}
