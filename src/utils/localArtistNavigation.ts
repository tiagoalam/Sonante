import type { LocalAlbum } from "../types/local";
import {
  compareLocalNavigationText,
  localNavigationBucket,
  normalizeLocalNavigationText,
  type LocalAlbumIndexBucket,
} from "./localAlbumNavigation.ts";

export interface LocalArtistSummary {
  id: string;
  name: string;
  albumCount: number;
}

export type LocalArtistSortMode = "artist-asc" | "artist-desc";

function deterministicDisplayName(left: string, right: string): string {
  const compared = compareLocalNavigationText(left, right);
  if (compared !== 0) return compared < 0 ? left : right;
  return left <= right ? left : right;
}

export function localArtistIdentity(name: string): string {
  return normalizeLocalNavigationText(name);
}

export function deriveLocalArtists(albums: readonly LocalAlbum[]): LocalArtistSummary[] {
  const grouped = new Map<string, LocalArtistSummary>();
  const albumIdsByArtist = new Map<string, Set<string>>();
  for (const album of albums) {
    const names = [...new Set([...(album.artist_names ?? []), album.artist].map(name => name.trim()).filter(Boolean))];
    for (const name of names) {
      const id = localArtistIdentity(name);
      if (!id) continue;
      const current = grouped.get(id);
      if (current) {
        const albumIds = albumIdsByArtist.get(id)!;
        if (!albumIds.has(album.id)) {
          albumIds.add(album.id);
          current.albumCount += 1;
        }
        current.name = deterministicDisplayName(current.name, name);
      } else {
        grouped.set(id, { id, name, albumCount: 1 });
        albumIdsByArtist.set(id, new Set([album.id]));
      }
    }
  }
  return [...grouped.values()];
}

export function filterAndSortLocalArtists(
  artists: readonly LocalArtistSummary[],
  query: string,
  mode: LocalArtistSortMode,
): LocalArtistSummary[] {
  const normalizedQuery = normalizeLocalNavigationText(query);
  const filtered = normalizedQuery
    ? artists.filter((artist) => normalizeLocalNavigationText(artist.name).includes(normalizedQuery))
    : [...artists];
  const direction = mode === "artist-asc" ? 1 : -1;
  return filtered.sort((left, right) => {
    const byName = compareLocalNavigationText(left.name, right.name) * direction;
    if (byName !== 0) return byName;
    return (left.id < right.id ? -1 : left.id > right.id ? 1 : 0) * direction;
  });
}

export function localArtistBucketFirstIndices(
  artists: readonly LocalArtistSummary[],
): Map<LocalAlbumIndexBucket, number> {
  const indices = new Map<LocalAlbumIndexBucket, number>();
  artists.forEach((artist, index) => {
    const bucket = localNavigationBucket(artist.name);
    if (!indices.has(bucket)) indices.set(bucket, index);
  });
  return indices;
}

export function albumsForLocalArtist(
  albums: readonly LocalAlbum[],
  artistName: string,
): LocalAlbum[] {
  const identity = localArtistIdentity(artistName);
  return albums.filter((album) => [album.artist, ...(album.artist_names ?? [])]
    .some(name => localArtistIdentity(name) === identity));
}
