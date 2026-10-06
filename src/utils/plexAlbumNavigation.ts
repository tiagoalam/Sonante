import type { PlexAlbum } from "../types/plex";
import {
  compareLocalNavigationText,
  localNavigationBucket,
  type LocalAlbumIndexBucket,
} from "./localAlbumNavigation.ts";

export type PlexAlbumSortMode =
  | "added_recent"
  | "album_asc"
  | "album_desc"
  | "artist_asc"
  | "artist_desc"
  | "year_desc"
  | "year_asc";

function compareRaw(left: string, right: string): number {
  if (left < right) return -1;
  if (left > right) return 1;
  return 0;
}

function compareTextFields(
  left: PlexAlbum,
  right: PlexAlbum,
  fields: Array<keyof Pick<PlexAlbum, "title" | "artist" | "year" | "rating_key">>,
  direction: 1 | -1,
): number {
  for (const field of fields) {
    const compared = compareLocalNavigationText(String(left[field] ?? ""), String(right[field] ?? ""));
    if (compared !== 0) return compared * direction;
  }
  return compareRaw(left.rating_key, right.rating_key) * direction;
}

export function parsePlexAlbumYear(
  year: number | undefined,
  currentYear = new Date().getFullYear(),
): number | null {
  return Number.isInteger(year) && year! >= 1000 && year! <= currentYear + 1 ? year! : null;
}

export function sortPlexAlbums(
  albums: readonly PlexAlbum[],
  mode: PlexAlbumSortMode,
  currentYear = new Date().getFullYear(),
): PlexAlbum[] {
  const sorted = [...albums];
  if (mode === "added_recent") return sorted;
  sorted.sort((left, right) => {
    switch (mode) {
      case "album_asc":
        return compareTextFields(left, right, ["title", "artist", "year", "rating_key"], 1);
      case "album_desc":
        return compareTextFields(left, right, ["title", "artist", "year", "rating_key"], -1);
      case "artist_asc":
        return compareTextFields(left, right, ["artist", "title", "year", "rating_key"], 1);
      case "artist_desc":
        return compareTextFields(left, right, ["artist", "title", "year", "rating_key"], -1);
      case "year_desc":
      case "year_asc": {
        const leftYear = parsePlexAlbumYear(left.year, currentYear);
        const rightYear = parsePlexAlbumYear(right.year, currentYear);
        if (leftYear === null && rightYear !== null) return 1;
        if (leftYear !== null && rightYear === null) return -1;
        if (leftYear !== null && rightYear !== null && leftYear !== rightYear) {
          return mode === "year_desc" ? rightYear - leftYear : leftYear - rightYear;
        }
        const byTitle = compareLocalNavigationText(left.title, right.title);
        return byTitle
          || compareLocalNavigationText(left.rating_key, right.rating_key)
          || compareRaw(left.rating_key, right.rating_key);
      }
    }
  });
  return sorted;
}

export function isTextPlexAlbumSort(mode: PlexAlbumSortMode): boolean {
  return mode === "album_asc"
    || mode === "album_desc"
    || mode === "artist_asc"
    || mode === "artist_desc";
}

export function plexAlbumBucket(
  album: PlexAlbum,
  mode: PlexAlbumSortMode,
): LocalAlbumIndexBucket {
  if (!isTextPlexAlbumSort(mode)) return "#";
  return localNavigationBucket(mode.startsWith("artist") ? album.artist : album.title);
}

export function plexAlbumBucketFirstIndices(
  albums: readonly PlexAlbum[],
  mode: PlexAlbumSortMode,
): Map<LocalAlbumIndexBucket, number> {
  const indices = new Map<LocalAlbumIndexBucket, number>();
  if (!isTextPlexAlbumSort(mode)) return indices;
  albums.forEach((album, index) => {
    const bucket = plexAlbumBucket(album, mode);
    if (!indices.has(bucket)) indices.set(bucket, index);
  });
  return indices;
}

export function plexAlbumNavigationResetKey(
  libraryKey: string | null,
  mode: PlexAlbumSortMode,
): string {
  return JSON.stringify([libraryKey, mode]);
}
