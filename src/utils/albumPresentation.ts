/** Presentation only: never changes catalog identity, sorting or playback. */
export const ALBUM_LAYOUTS = ['grid', 'list', 'details'] as const;
export const ALBUM_COVER_SIZES = ['small', 'medium', 'large'] as const;
export type AlbumLayout = typeof ALBUM_LAYOUTS[number];
export type AlbumCoverSize = typeof ALBUM_COVER_SIZES[number];
export interface AlbumPresentation { layout: AlbumLayout; coverSize: AlbumCoverSize }
export const DEFAULT_ALBUM_PRESENTATION: AlbumPresentation = { layout: 'grid', coverSize: 'medium' };
export const ALBUM_CARD_WIDTHS: Record<AlbumCoverSize, number> = { small: 130, medium: 170, large: 230 };
export function albumPresentation(value: unknown): AlbumPresentation {
  if (!value || typeof value !== 'object') return { ...DEFAULT_ALBUM_PRESENTATION };
  const v=value as Partial<AlbumPresentation>;
  return { layout: ALBUM_LAYOUTS.includes(v.layout as AlbumLayout) ? v.layout! : 'grid',
    coverSize: ALBUM_COVER_SIZES.includes(v.coverSize as AlbumCoverSize) ? v.coverSize! : 'medium' };
}
export function albumPresentationKey(source: 'local' | 'plex' | 'navidrome') { return `sonante.album-presentation.v1.${source}`; }
export function readAlbumPresentation(source: 'local' | 'plex' | 'navidrome'): AlbumPresentation {
  try { return albumPresentation(JSON.parse(globalThis.localStorage?.getItem(albumPresentationKey(source)) ?? 'null')); }
  catch { return { ...DEFAULT_ALBUM_PRESENTATION }; }
}
export function saveAlbumPresentation(source: 'local' | 'plex' | 'navidrome', value: AlbumPresentation): void {
  try { globalThis.localStorage?.setItem(albumPresentationKey(source), JSON.stringify(albumPresentation(value))); }
  catch { /* A blocked/full store must not prevent changing the current view. */ }
}
export function albumAddedDate(value: string | null | undefined, locale: string): string | null {
  if (!value || !/^\d{4}-\d{2}-\d{2}(?:T|$)/.test(value)) return null;
  const date=new Date(value); if (!Number.isFinite(date.getTime())) return null;
  return date.toLocaleDateString(locale, { year: 'numeric', month: 'short', day: 'numeric' });
}
