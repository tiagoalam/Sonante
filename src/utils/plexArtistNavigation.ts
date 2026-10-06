import type { PlexAlbum } from "../types/plex";
import {
  compareLocalNavigationText,
  localNavigationBucket,
  normalizeLocalNavigationText,
  type LocalAlbumIndexBucket,
} from "./localAlbumNavigation.ts";

export interface PlexArtistSummary {
  id: string;
  ratingKey?: string;
  name: string;
  albumCount: number;
}

export type PlexArtistSortMode = "artist_asc" | "artist_desc";

function deterministicDisplayName(left: string, right: string): string {
  const compared = compareLocalNavigationText(left, right);
  if (compared !== 0) return compared < 0 ? left : right;
  return left <= right ? left : right;
}

function fallbackArtistId(name: string): string {
  return `name:${normalizeLocalNavigationText(name)}`;
}

export function plexArtistIdentity(album: PlexAlbum): string | null {
  const ratingKey = album.artist_rating_key?.trim();
  if (ratingKey) return `rating:${ratingKey}`;
  const normalizedName = normalizeLocalNavigationText(album.artist);
  return normalizedName ? fallbackArtistId(album.artist) : null;
}

export function derivePlexArtists(albums: readonly PlexAlbum[]): PlexArtistSummary[] {
  const grouped = new Map<string, PlexArtistSummary>();
  for (const album of albums) {
    const name = album.artist.trim();
    const id = plexArtistIdentity(album);
    if (!id || !name) continue;
    const current = grouped.get(id);
    if (current) {
      current.albumCount += 1;
      current.name = deterministicDisplayName(current.name, name);
    } else {
      const ratingKey = album.artist_rating_key?.trim() || undefined;
      grouped.set(id, ratingKey
        ? { id, ratingKey, name, albumCount: 1 }
        : { id, name, albumCount: 1 });
    }
  }
  return [...grouped.values()];
}

export function sortPlexArtists(
  artists: readonly PlexArtistSummary[],
  mode: PlexArtistSortMode,
): PlexArtistSummary[] {
  const direction = mode === "artist_asc" ? 1 : -1;
  return [...artists].sort((left, right) => {
    const byName = compareLocalNavigationText(left.name, right.name) * direction;
    if (byName !== 0) return byName;
    return (left.id < right.id ? -1 : left.id > right.id ? 1 : 0) * direction;
  });
}

export function plexArtistBucketFirstIndices(
  artists: readonly PlexArtistSummary[],
): Map<LocalAlbumIndexBucket, number> {
  const indices = new Map<LocalAlbumIndexBucket, number>();
  artists.forEach((artist, index) => {
    const bucket = localNavigationBucket(artist.name);
    if (!indices.has(bucket)) indices.set(bucket, index);
  });
  return indices;
}

export function albumsForPlexArtist(
  albums: readonly PlexAlbum[],
  artist: PlexArtistSummary,
): PlexAlbum[] {
  return albums.filter((album) => plexArtistIdentity(album) === artist.id);
}
