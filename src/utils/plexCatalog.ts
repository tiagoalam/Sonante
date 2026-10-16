import type { PlexAlbum, PlexAlbumCatalog } from "../types/plex.ts";

// The browser only accepts a verified complete catalog; partial/unknown replies
// are never converted into an empty or apparently successful album list.
export function requireCompletePlexCatalog(catalog: PlexAlbumCatalog, libraryId: string): PlexAlbum[] {
  if (!catalog || catalog.completeness !== "complete" || catalog.library_id !== libraryId
    || !catalog.server_id || !Array.isArray(catalog.albums)
    || !Number.isSafeInteger(catalog.total) || catalog.total !== catalog.albums.length) {
    throw new Error("Catálogo Plex sem completude ou contexto confirmado.");
  }
  return catalog.albums;
}
