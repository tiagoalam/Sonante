import type { CurrentMedia } from "../types/audio";
import { localAlbumPresentationContext } from "./localAlbumPresentationCache.ts";
import { thumbnailFromLoadedImage, validThumbnail } from "./localAlbumThumbnails.ts";

export const LOCAL_PLAYBACK_THUMBNAIL_KEY = "sonante.localPlaybackThumbnail.v1";
export const MAX_PLAYBACK_THUMBNAIL_RECORD = 96 * 1024;
type CacheStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;
function localUri(media: CurrentMedia | null): string | null {
  if (media?.kind !== "local" || !media.uri || media.uri.length > 8192
    || /[\0\r\n]/.test(media.uri) || media.uri.includes("://")
    || media.uri.split("/").some(part => !part || part === "." || part === "..")) return null;
  return media.uri.includes("/") ? media.uri : null;
}

// One optional, bounded presentation preview. Never playback/source authority,
// a queue cache, a full-resolution artwork cache or a reason to skip validation.
export class LocalPlaybackThumbnail {
  private context: string | null = null;
  private generation = 0;
  private captured = new WeakMap<HTMLImageElement, { uri: string; source: string; context: string }>();
  private entry: { uri: string; thumb: string } | null = null;
  private readonly storage: () => CacheStorage;
  private readonly reduce: typeof thumbnailFromLoadedImage;
  constructor(storage: () => CacheStorage, reduce = thumbnailFromLoadedImage) {
    this.storage = storage; this.reduce = reduce;
  }

  setContext(folders: readonly string[]): void {
    const context = localAlbumPresentationContext(folders);
    if (context === this.context) return;
    this.context = context; this.generation++; this.entry = null;
    if (context === null) return;
    try {
      const raw = this.storage().getItem(LOCAL_PLAYBACK_THUMBNAIL_KEY);
      if (!raw || raw.length > MAX_PLAYBACK_THUMBNAIL_RECORD
        || new TextEncoder().encode(raw).byteLength > MAX_PLAYBACK_THUMBNAIL_RECORD) return;
      const record: unknown = JSON.parse(raw);
      if (!record || typeof record !== "object" || Array.isArray(record)) return;
      const value = record as Record<string, unknown>;
      if (value.version !== 1 || value.context !== context || typeof value.uri !== "string"
        || localUri({ kind: "local", uri: value.uri, queue_index: 0 }) === null
        || !validThumbnail(value.thumb)) return;
      this.entry = { uri: value.uri, thumb: value.thumb };
    } catch { /* Optional presentation; a miss never blocks playback. */ }
  }

  get(media: CurrentMedia | null): string | null {
    const uri = localUri(media);
    return this.context !== null && uri !== null && this.entry?.uri === uri ? this.entry.thumb : null;
  }

  capture(media: CurrentMedia | null, image: HTMLImageElement, current: () => boolean): void {
    const uri = localUri(media), context = this.context, generation = this.generation;
    if (context === null || uri === null || !current()) return;
    const source = image.currentSrc || image.src;
    // Do not recapture our own small preview as if it were a fresh image.
    if (this.entry?.uri === uri && source === this.entry.thumb) return;
    const previous = this.captured.get(image);
    if (previous?.uri === uri && previous.source === source && previous.context === context) return;
    const thumb = validThumbnail(source) ? source : this.reduce(image);
    if (!thumb || !validThumbnail(thumb) || !current() || generation !== this.generation) return;
    this.captured.set(image, { uri, source, context });
    if (this.entry?.uri === uri && this.entry.thumb === thumb) return;
    this.entry = { uri, thumb };
    const record = JSON.stringify({ version: 1, context, uri, thumb });
    if (new TextEncoder().encode(record).byteLength > MAX_PLAYBACK_THUMBNAIL_RECORD) return;
    try { this.storage().setItem(LOCAL_PLAYBACK_THUMBNAIL_KEY, record); }
    catch { /* Optional cache; no retry, logging of media paths or runtime action. */ }
  }

  forget(media: CurrentMedia | null, failed: string): void {
    if (this.get(media) !== failed) return;
    this.entry = null;
    try { this.storage().removeItem(LOCAL_PLAYBACK_THUMBNAIL_KEY); } catch { /* Optional. */ }
  }
}

export const localPlaybackThumbnail = new LocalPlaybackThumbnail(() => window.localStorage);
