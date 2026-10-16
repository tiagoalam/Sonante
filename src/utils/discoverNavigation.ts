import type { PlexAlbum, SelectedArtist } from "../types/plex.ts";
export type DiscoverPage = { identity: unknown; album: PlexAlbum; artist?: never } | { identity: unknown; artist: SelectedArtist; album?: never };
export function discoverPush(pages: DiscoverPage[], page: DiscoverPage): DiscoverPage[] {
  const prior = pages[pages.length - 2];
  // An album's artist link returns to the already mounted parent ArtistView.
  if (page.artist && prior?.artist && page.identity === prior.identity &&
      page.artist.server_id === prior.artist.server_id && page.artist.library_id === prior.artist.library_id &&
      page.artist.rating_key === prior.artist.rating_key) return pages.slice(0, -1);
  return [...pages, page];
}
