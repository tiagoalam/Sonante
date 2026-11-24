export function carouselWindow(length: number, index: number): { selected: number; start: number; end: number };
export function carouselKeyDirection(key: string): number;
export function carouselSwipeDirection(start: number | null, end: number | undefined): number;
export function carouselCoverSize(stageWidth: number, stageHeight: number, cardExtraHeight?: number): number;
