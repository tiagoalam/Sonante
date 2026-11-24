export function carouselWindow(length, index) {
  const count = Math.max(0, Math.floor(Number.isFinite(length) ? length : 0));
  const selected = count ? Math.max(0, Math.min(count - 1, Math.floor(Number.isFinite(index) ? index : 0))) : 0;
  return { selected, start: Math.max(0, selected - 2), end: Math.min(count, selected + 3) };
}
export function carouselKeyDirection(key) { return key === 'ArrowRight' ? 1 : key === 'ArrowLeft' ? -1 : 0; }
export function carouselSwipeDirection(start, end) { return Number.isFinite(start) && Number.isFinite(end) && Math.abs(end - start) > 50 ? (end < start ? 1 : -1) : 0; }

// Keep the approved desktop cover size across source pages; shrink only when
// the actual stage width or the card's remaining vertical space requires it.
export function carouselCoverSize(stageWidth, stageHeight, cardExtraHeight = 30) {
  const width = Number.isFinite(stageWidth) ? Math.max(0, Math.floor(stageWidth)) : 0;
  const height = Number.isFinite(stageHeight) ? Math.max(0, Math.floor(stageHeight)) : 0;
  const extra = Number.isFinite(cardExtraHeight) ? Math.max(0, Math.floor(cardExtraHeight)) : 0;
  return Math.floor(Math.min(328, width, Math.max(0, height - extra)));
}
