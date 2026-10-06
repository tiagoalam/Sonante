import type { LocalAlbum } from "../types/local";

export type LocalAlbumSortMode =
  | "album-asc"
  | "album-desc"
  | "artist-asc"
  | "artist-desc"
  | "year-newest"
  | "year-oldest";

export const LOCAL_ALBUM_INDEX_BUCKETS = [
  "#", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
  "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
] as const;

export type LocalAlbumIndexBucket = typeof LOCAL_ALBUM_INDEX_BUCKETS[number];

const textCollator = new Intl.Collator("und", {
  sensitivity: "base",
  numeric: true,
  usage: "sort",
});

export function compareLocalNavigationText(left: string, right: string): number {
  return textCollator.compare(left, right);
}

function compareRaw(left: string, right: string): number {
  if (left < right) return -1;
  if (left > right) return 1;
  return 0;
}

function compareTextFields(
  left: LocalAlbum,
  right: LocalAlbum,
  fields: Array<keyof Pick<LocalAlbum, "title" | "artist" | "year" | "id">>,
  direction: 1 | -1,
): number {
  for (const field of fields) {
    const compared = compareLocalNavigationText(left[field] ?? "", right[field] ?? "");
    if (compared !== 0) return compared * direction;
  }
  return compareRaw(left.id, right.id) * direction;
}

export function parseLocalAlbumYear(
  year: string | undefined,
  currentYear = new Date().getFullYear(),
): number | null {
  if (!year || !/^\d{4}$/.test(year)) return null;
  const parsed = Number(year);
  return parsed >= 1000 && parsed <= currentYear + 1 ? parsed : null;
}

export function sortLocalAlbums(
  albums: readonly LocalAlbum[],
  mode: LocalAlbumSortMode,
  currentYear = new Date().getFullYear(),
): LocalAlbum[] {
  const sorted = [...albums];
  sorted.sort((left, right) => {
    switch (mode) {
      case "album-asc":
        return compareTextFields(left, right, ["title", "artist", "year", "id"], 1);
      case "album-desc":
        return compareTextFields(left, right, ["title", "artist", "year", "id"], -1);
      case "artist-asc":
        return compareTextFields(left, right, ["artist", "title", "year", "id"], 1);
      case "artist-desc":
        return compareTextFields(left, right, ["artist", "title", "year", "id"], -1);
      case "year-newest":
      case "year-oldest": {
        const leftYear = parseLocalAlbumYear(left.year, currentYear);
        const rightYear = parseLocalAlbumYear(right.year, currentYear);
        if (leftYear === null && rightYear !== null) return 1;
        if (leftYear !== null && rightYear === null) return -1;
        if (leftYear !== null && rightYear !== null && leftYear !== rightYear) {
          return mode === "year-newest" ? rightYear - leftYear : leftYear - rightYear;
        }
        const byTitle = compareLocalNavigationText(left.title, right.title);
        return byTitle || compareLocalNavigationText(left.id, right.id) || compareRaw(left.id, right.id);
      }
    }
  });
  return sorted;
}

export function isTextLocalAlbumSort(mode: LocalAlbumSortMode): boolean {
  return mode === "album-asc"
    || mode === "album-desc"
    || mode === "artist-asc"
    || mode === "artist-desc";
}

export function normalizeLocalNavigationText(value: string): string {
  return value
    .trim()
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLocaleLowerCase("und");
}

export function localNavigationBucket(value: string): LocalAlbumIndexBucket {
  const first = normalizeLocalNavigationText(value).toUpperCase().charAt(0);
  return /^[A-Z]$/.test(first) ? first as LocalAlbumIndexBucket : "#";
}

export function localAlbumIndexBucket(
  album: LocalAlbum,
  mode: LocalAlbumSortMode,
): LocalAlbumIndexBucket {
  if (!isTextLocalAlbumSort(mode)) return "#";
  const value = mode.startsWith("artist") ? album.artist : album.title;
  return localNavigationBucket(value);
}

export function localAlbumBucketFirstIndices(
  albums: readonly LocalAlbum[],
  mode: LocalAlbumSortMode,
): Map<LocalAlbumIndexBucket, number> {
  const indices = new Map<LocalAlbumIndexBucket, number>();
  if (!isTextLocalAlbumSort(mode)) return indices;
  albums.forEach((album, index) => {
    const bucket = localAlbumIndexBucket(album, mode);
    if (!indices.has(bucket)) indices.set(bucket, index);
  });
  return indices;
}

export function localAlbumNavigationResetKey(
  selectedSourceId: string | null,
  searchQuery: string,
  sortMode: LocalAlbumSortMode,
): string {
  return JSON.stringify([selectedSourceId, searchQuery, sortMode]);
}
