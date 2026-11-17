import type { EditionSource } from './albumEditions';

const currentContext = new Map<EditionSource, object>();
const verifiedAlbumIds = new Map<EditionSource, Set<string>>();

/** Start a fresh, source-specific evidence snapshot without doing any reads. */
export function activateEditionAvailability(source: EditionSource, context: object): void {
  if (currentContext.get(source) === context) return;
  currentContext.set(source, context);
  verifiedAlbumIds.set(source, new Set());
}

/** Publish only album identities from already verified edition groups. */
export function publishVerifiedEditionAvailability(source: EditionSource, context: object, albumIds: readonly string[]): void {
  if (currentContext.get(source) !== context) return;
  verifiedAlbumIds.set(source, new Set(albumIds.filter(Boolean)));
}

export function hasVerifiedEditionAlternative(source: EditionSource, albumId: string): boolean {
  return !!albumId && (verifiedAlbumIds.get(source)?.has(albumId) ?? false);
}

export function shouldShowAlbumEditionsAction(experimentalEnabled: boolean, validAlbum: boolean, hasAlternative: boolean): boolean {
  return experimentalEnabled && validAlbum && hasAlternative;
}
