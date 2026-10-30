import type { LocalAlbum } from "../types/local";
import { compareLocalAlbumsAscending } from "./localAlbumNavigation.ts";

// One replaceable, presentation-only record per webview origin. Never persist
// AppConfig, artwork, track lists, credentials or a playback confirmation here.
export const LOCAL_ALBUM_PRESENTATION_KEY = "sonante.localAlbumPresentation.v1";
export const MAX_PRESENTATION_BYTES = 2 * 1024 * 1024;
export const MAX_PRESENTATION_ALBUMS = 10_000;
export const MAX_PRESENTATION_PREVIEW_ALBUMS = 250;
const VERSION = 1;
const MAX_CONTEXT_LENGTH = 32_768;
type CacheStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;
export interface LocalAlbumPresentationCache {
  read(context: string): LocalAlbum[] | null;
  write(context: string, albums: readonly LocalAlbum[]): void;
}

export function localAlbumPresentationContext(folders: readonly string[]): string | null {
  if (!Array.isArray(folders) || !folders.length || folders.length > 256
    || !folders.every(folder => typeof folder === "string" && folder.startsWith("/") && folder.length <= 8192)) return null;
  // Exact configured paths/order: a change is a cache miss, never an inferred
  // source identity. The browser origin supplies the app/storage scope.
  const context = JSON.stringify(folders);
  return context.length <= MAX_CONTEXT_LENGTH ? context : null;
}

const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && value.length <= 8192;
const count = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const optionalText = (value: unknown) => value == null || text(value);

// Validate and project the same allowlist for both reads and writes.
function projectAlbum(item: unknown): LocalAlbum | null {
  if (!object(item) || !text(item.id) || !item.id
    || !text(item.title) || !text(item.artist) || !text(item.folder_path)
    || !count(item.track_count) || !optionalText(item.year)
    || !optionalText(item.source_id) || !optionalText(item.source_label)
    || !(item.genres == null || Array.isArray(item.genres) && item.genres.length <= 128 && item.genres.every(text))
    || !Array.isArray(item.discs) || item.discs.length > 256) return null;
  const discs: LocalAlbum["discs"] = [];
  for (const disc of item.discs) {
    if (!object(disc) || !count(disc.number) || !text(disc.label)
      || !text(disc.folder_path) || !count(disc.track_count)) return null;
    discs.push({ number: disc.number, label: disc.label, folder_path: disc.folder_path, track_count: disc.track_count });
  }
  return { id: item.id, title: item.title, artist: item.artist,
    folder_path: item.folder_path, track_count: item.track_count, discs,
    ...(typeof item.year === "string" ? { year: item.year } : {}),
    ...(Array.isArray(item.genres) ? { genres: [...item.genres] as string[] } : {}),
    ...(typeof item.source_id === "string" ? { source_id: item.source_id } : {}),
    ...(typeof item.source_label === "string" ? { source_label: item.source_label } : {}),
  };
}

function projectAlbums(value: unknown): LocalAlbum[] | null {
  if (!Array.isArray(value) || value.length > MAX_PRESENTATION_ALBUMS) return null;
  const ids = new Set<string>();
  const albums: LocalAlbum[] = [];
  let projectedBytes = 0;
  for (const item of value) {
    const album = projectAlbum(item);
    if (album === null || ids.has(album.id)) return null;
    ids.add(album.id);
    albums.push(album);
    projectedBytes += new TextEncoder().encode(JSON.stringify(album)).byteLength;
    if (projectedBytes > MAX_PRESENTATION_BYTES) return null;
  }
  return albums;
}

// Max-heap selection: O(N log K) comparisons and O(K) retained candidates,
// then sort only those K rows. The comparator is identical to album-asc.
export function selectLocalAlbumPresentationPreview(
  albums: Iterable<LocalAlbum>,
  compare: (left: LocalAlbum, right: LocalAlbum) => number = compareLocalAlbumsAscending,
): LocalAlbum[] {
  const heap: LocalAlbum[] = [];
  for (const album of albums) {
    if (heap.length < MAX_PRESENTATION_PREVIEW_ALBUMS) {
      let index = heap.push(album) - 1;
      while (index > 0) {
        const parent = Math.floor((index - 1) / 2);
        if (compare(heap[index], heap[parent]) <= 0) break;
        [heap[index], heap[parent]] = [heap[parent], heap[index]];
        index = parent;
      }
    } else if (compare(album, heap[0]) < 0) {
      heap[0] = album;
      let index = 0;
      while (index * 2 + 1 < heap.length) {
        let child = index * 2 + 1;
        if (child + 1 < heap.length && compare(heap[child + 1], heap[child]) > 0) child += 1;
        if (compare(heap[index], heap[child]) >= 0) break;
        [heap[index], heap[child]] = [heap[child], heap[index]];
        index = child;
      }
    }
  }
  return heap.sort(compare);
}

function projectForWrite(context: string, value: unknown): LocalAlbum[] | null {
  if (!Array.isArray(value)) return null;
  const input = value;
  const encoder = new TextEncoder();
  const envelopeBytes = encoder.encode(JSON.stringify({ version: VERSION, context, albums: [] })).byteLength;
  let complete: LocalAlbum[] | null = input.length <= MAX_PRESENTATION_ALBUMS ? [] : null;
  let completeBytes = envelopeBytes;
  let valid = true;
  // Full validation keeps O(N) ID references to reject duplicates even outside
  // the retained preview. It does not keep N projected metadata objects.
  const ids = new Set<string>();
  function* validatedAlbums(): Generator<LocalAlbum> {
    for (const item of input) {
      const album = projectAlbum(item);
      if (album === null || ids.has(album.id)) { valid = false; return; }
      ids.add(album.id);
      if (complete !== null) {
        completeBytes += encoder.encode(JSON.stringify(album)).byteLength + (complete.length ? 1 : 0);
        if (completeBytes <= MAX_PRESENTATION_BYTES) complete.push(album);
        else complete = null;
      }
      // Keep only references to input rows in the heap, not K potentially
      // large projected copies. The validated sort fields project unchanged.
      yield item as LocalAlbum;
    }
  }
  const first = selectLocalAlbumPresentationPreview(validatedAlbums());
  if (!valid) return null; // Invalid input never becomes a valid partial snapshot.
  if (complete !== null) return complete; // Preserve every fitting snapshot, including its order.
  const preview: LocalAlbum[] = [];
  let bytes = envelopeBytes;
  for (const item of first) {
    const album = projectAlbum(item);
    if (album === null) return null;
    const nextBytes = encoder.encode(JSON.stringify(album)).byteLength + (preview.length ? 1 : 0);
    if (bytes + nextBytes > MAX_PRESENTATION_BYTES) break;
    preview.push(album);
    bytes += nextBytes;
  }
  // Preserve an exact opening prefix: never skip an oversized first card.
  return preview.length ? preview : null;
}

function withinLimit(serialized: string): boolean {
  return serialized.length <= MAX_PRESENTATION_BYTES && new TextEncoder().encode(serialized).byteLength <= MAX_PRESENTATION_BYTES;
}

export function createLocalAlbumPresentationCache(storage: () => CacheStorage): LocalAlbumPresentationCache {
  return {
    read(context) {
      try {
        const serialized = storage().getItem(LOCAL_ALBUM_PRESENTATION_KEY);
        if (!serialized || !withinLimit(serialized)) return null;
        const record: unknown = JSON.parse(serialized);
        if (!object(record) || record.version !== VERSION || record.context !== context) return null;
        return projectAlbums(record.albums);
      } catch { return null; } // Optional presentation must never block startup.
    },
    write(context, albums) {
      try {
        const projected = projectForWrite(context, albums);
        const serialized = projected === null ? null : JSON.stringify({ version: VERSION, context, albums: projected });
        if (serialized !== null && withinLimit(serialized)) {
          storage().setItem(LOCAL_ALBUM_PRESENTATION_KEY, serialized);
          return;
        }
      } catch { /* Quota/security failure falls back to the normal catalog read. */ }
      try { storage().removeItem(LOCAL_ALBUM_PRESENTATION_KEY); } catch { /* Storage may be unavailable. */ }
    },
  };
}
