import type { LocalAlbum } from "../types/local";
import { localAlbumPresentationContext, MAX_PRESENTATION_ALBUMS } from "./localAlbumPresentationCache.ts";

export const LOCAL_ALBUM_THUMBNAILS_KEY = "sonante.localAlbumThumbnails.v1";
export const MAX_THUMBNAILS = 250;
export const MAX_THUMBNAIL_BYTES = 24 * 1024;
export const MAX_THUMBNAILS_BYTES = 1024 * 1024;
export const THUMBNAIL_EDGE = 160;
type ThumbnailStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;
type Schedule = (task: () => void) => () => void;
type RejectReason = "context" | "admission" | "key" | "image_incomplete" | "source_type" | "source_size"
  | "canvas_unavailable" | "canvas_failed" | "jpeg_invalid" | "thumbnail_size" | "reduction_failed" | "changed" | "capacity";
type RestoreReason = "absent" | "restored" | "disabled" | "oversized" | "invalid_record" | "context_mismatch" | "invalid_entry" | "storage_error";
type Trigger = "on_load" | "already_complete";
export interface ThumbnailDiagnostic {
  stage: "restore" | "summary" | "final";
  restore: RestoreReason;
  restored: number; captured: number; stored: number; retained: number; writes: number; write_failures: number;
  on_load: number; already_complete: number; initial_cards: number; initial_hits: number; initial_misses: number;
  evicted_count: number; evicted_bytes: number; rejected: Partial<Record<RejectReason, number>>;
}
const bytes = (value: string) => new TextEncoder().encode(value).byteLength;
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);

function albumKey(album: LocalAlbum): string | null {
  const key = JSON.stringify([album.id, album.source_id ?? null, album.folder_path,
    album.title, album.artist, album.discs.map(disc => [disc.number, disc.folder_path])]);
  return key.length <= 32_768 ? key : null;
}

// Only bounded JPEG data, including an actual small SOF frame. Never accept
// SVG, remote/file/asset URLs or an unbounded image hidden behind a data URL.
function validThumbnail(value: unknown): value is string {
  if (typeof value !== "string" || value.length > MAX_THUMBNAIL_BYTES
    || !/^data:image\/jpeg;base64,[A-Za-z0-9+/]+={0,2}$/.test(value)) return false;
  try {
    const jpeg = atob(value.slice("data:image/jpeg;base64,".length));
    const u8 = (i: number) => jpeg.charCodeAt(i);
    if (u8(0) !== 255 || u8(1) !== 216 || u8(jpeg.length - 2) !== 255 || u8(jpeg.length - 1) !== 217) return false;
    let offset = 2;
    while (offset + 4 < jpeg.length) {
      if (u8(offset++) !== 255) return false;
      while (u8(offset) === 255) offset++;
      const marker = u8(offset++);
      const length = u8(offset) * 256 + u8(offset + 1);
      if (length < 2 || offset + length > jpeg.length) return false;
      if ([192, 193, 194].includes(marker)) {
        if (length < 8) return false;
        const height = u8(offset + 3) * 256 + u8(offset + 4);
        const width = u8(offset + 5) * 256 + u8(offset + 6);
        return height > 0 && width > 0 && height <= THUMBNAIL_EDGE && width <= THUMBNAIL_EDGE;
      }
      if (marker === 218) return false;
      offset += length;
    }
  } catch { /* Corrupt optional image. */ }
  return false;
}

// Draw the image already decoded by the normal card. No Image(), fetch(), MPD
// or filesystem lookup is involved; only the reduced JPEG is retained.
export function thumbnailFromLoadedImage(image: HTMLImageElement,
  makeCanvas: () => HTMLCanvasElement = () => document.createElement("canvas"),
  reject: (reason: RejectReason) => void = () => {}): string | null {
  const source = image.currentSrc || image.src;
  if (!image.complete || !image.naturalWidth || !image.naturalHeight) { reject("image_incomplete"); return null; }
  if (source.length > 24 * 1024 * 1024) { reject("source_size"); return null; }
  if (!/^data:image\/(?:jpeg|png|webp);base64,/.test(source)) { reject("source_type"); return null; }
  try {
    const scale = Math.min(1, THUMBNAIL_EDGE / Math.max(image.naturalWidth, image.naturalHeight));
    const canvas = makeCanvas();
    canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const context = canvas.getContext("2d");
    if (!context) { reject("canvas_unavailable"); return null; }
    context.fillStyle = "#202020";
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    const result = canvas.toDataURL("image/jpeg", 0.7);
    if (result.length > MAX_THUMBNAIL_BYTES) { reject("thumbnail_size"); return null; }
    if (!validThumbnail(result)) { reject("jpeg_invalid"); return null; }
    return result;
  } catch { reject("canvas_failed"); return null; }
}

export class LocalAlbumThumbnails {
  private context: string | null = null;
  private folders: readonly string[] | null = null;
  private generation = 0;
  private images = new Map<string, string>();
  private ranks = new Map<string, number>();
  private observedInitial = new Set<string>();
  private capturedImages = new WeakMap<HTMLImageElement, { key: string; source: string }>();
  private pending: string | null = null;
  private cancelScheduled?: () => void;
  private cancelDiagnostic?: () => void;
  private reports = 0;
  private finalReported = false;
  private stats = this.emptyStats();
  private readonly storage: () => ThumbnailStorage;
  private readonly reduce: typeof thumbnailFromLoadedImage;
  private readonly schedule: Schedule;
  private readonly emit?: (value: ThumbnailDiagnostic) => void;
  constructor(storage: () => ThumbnailStorage, reduce = thumbnailFromLoadedImage,
    schedule: Schedule = task => { const timer = setTimeout(task, 250); return () => clearTimeout(timer); },
    emit?: (value: ThumbnailDiagnostic) => void) {
    this.storage = storage; this.reduce = reduce; this.schedule = schedule; this.emit = emit;
  }

  private emptyStats(): Omit<ThumbnailDiagnostic, "stage" | "retained"> {
    return { restore: "absent", restored: 0, captured: 0, stored: 0, writes: 0, write_failures: 0,
      on_load: 0, already_complete: 0, initial_cards: 0, initial_hits: 0, initial_misses: 0,
      evicted_count: 0, evicted_bytes: 0, rejected: {} };
  }

  setContext(folders: readonly string[]): void {
    const previous = this.folders;
    if (previous !== null && folders.length === previous.length && folders.every((folder, i) => folder === previous[i])) return;
    this.cancelScheduled?.(); this.cancelScheduled = undefined; this.pending = null;
    this.cancelDiagnostic?.(); this.cancelDiagnostic = undefined;
    this.generation++; this.folders = [...folders]; this.images.clear(); this.ranks.clear(); this.observedInitial.clear();
    this.capturedImages = new WeakMap(); this.stats = this.emptyStats();
    this.context = localAlbumPresentationContext(folders);
    this.stats.restore = this.restore();
    this.report("restore");
  }

  private restore(): RestoreReason {
    if (this.context === null) return "disabled";
    try {
      const raw = this.storage().getItem(LOCAL_ALBUM_THUMBNAILS_KEY);
      if (!raw) return "absent";
      if (raw.length > MAX_THUMBNAILS_BYTES || bytes(raw) > MAX_THUMBNAILS_BYTES) return "oversized";
      let record: unknown;
      try { record = JSON.parse(raw); } catch { return "invalid_record"; }
      if (!object(record) || record.version !== 1 || !Array.isArray(record.images) || record.images.length > MAX_THUMBNAILS) return "invalid_record";
      if (record.context !== this.context) return "context_mismatch";
      const restored = new Map<string, string>();
      for (const entry of record.images) {
        if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== "string" || entry[0].length > 32_768
          || restored.has(entry[0]) || !validThumbnail(entry[1])) return "invalid_entry";
        restored.set(entry[0], entry[1]);
      }
      this.images = restored; this.stats.restored = restored.size;
      return "restored";
    } catch { return "storage_error"; }
  }

  // Metadata already available to the browser, in its actual default opening
  // order. No cover lookup, storage write or preload is caused by this policy.
  setOpeningOrder(albums: readonly LocalAlbum[]): void {
    this.ranks = new Map(albums.slice(0, MAX_PRESENTATION_ALBUMS).map((album, index) => [album.id, index]));
  }

  private rank(key: string): number {
    try {
      const value: unknown = JSON.parse(key);
      if (Array.isArray(value) && typeof value[0] === "string") return this.ranks.get(value[0]) ?? Number.MAX_SAFE_INTEGER;
    } catch { /* An unmatched old entry has no opening priority. */ }
    return Number.MAX_SAFE_INTEGER;
  }

  // Only the DEV observer calls this effect for mounted cards near the start.
  // It changes finite RAM counters, never cache storage or product admission.
  observeInitialCard(album: LocalAlbum): void {
    if (!this.emit || this.observedInitial.size >= MAX_THUMBNAILS || (this.ranks.get(album.id) ?? MAX_THUMBNAILS) >= MAX_THUMBNAILS || this.observedInitial.has(album.id)) return;
    this.observedInitial.add(album.id); this.stats.initial_cards++;
    if (this.get(album)) this.stats.initial_hits++; else this.stats.initial_misses++;
    this.scheduleDiagnostic();
  }

  get(album: LocalAlbum): string | null {
    const key = albumKey(album);
    return this.context !== null && key !== null ? this.images.get(key) ?? null : null;
  }

  capture(album: LocalAlbum, image: HTMLImageElement, current: () => boolean, trigger: Trigger = "on_load"): void {
    if (trigger === "already_complete") this.stats.already_complete++; else this.stats.on_load++;
    if (this.context === null) { this.reject("context"); return; }
    if (!current()) { this.reject("admission"); return; }
    const generation = this.generation, key = albumKey(album);
    if (key === null) { this.reject("key"); return; }
    const source = image.currentSrc || image.src;
    const previous = this.capturedImages.get(image);
    if (previous?.key === key && previous.source === source && this.images.has(key)) return;
    let reason: RejectReason = "reduction_failed";
    const thumbnail = this.reduce(image, undefined, value => { reason = value; });
    if (thumbnail === null) { this.reject(reason); return; }
    if (!validThumbnail(thumbnail)) { this.reject("jpeg_invalid"); return; }
    if (generation !== this.generation || !current()) { this.reject("changed"); return; }
    this.capturedImages.set(image, { key, source });
    if (this.images.get(key) === thumbnail) return;
    // Replace a prior metadata key for the same album instead of retaining both.
    for (const existing of this.images.keys()) {
      try { if (JSON.parse(existing)[0] === album.id) this.images.delete(existing); } catch { /* Unmatched. */ }
    }
    this.images.set(key, thumbnail); this.stats.captured++;
    this.queueWrite();
    if (!this.images.has(key)) this.reject("capacity");
  }

  forget(album: LocalAlbum, current: () => boolean): void {
    const key = albumKey(album);
    if (this.context !== null && key !== null && current() && this.images.delete(key)) this.queueWrite();
  }

  private queueWrite(): void {
    const generation = this.generation;
    this.images = new Map([...this.images].sort(([a], [b]) => this.rank(a) - this.rank(b) || (a < b ? -1 : a > b ? 1 : 0)));
    const record = () => JSON.stringify({ version: 1, context: this.context, images: [...this.images] });
    let serialized = record();
    while (this.images.size > MAX_THUMBNAILS || bytes(serialized) > MAX_THUMBNAILS_BYTES) {
      if (this.images.size > MAX_THUMBNAILS) this.stats.evicted_count++; else this.stats.evicted_bytes++;
      const last = [...this.images.keys()].pop();
      if (last === undefined) return;
      this.images.delete(last); serialized = record();
    }
    this.pending = serialized;
    if (!this.cancelScheduled) this.cancelScheduled = this.schedule(() => {
      if (generation !== this.generation) return;
      this.cancelScheduled = undefined; this.flush();
    });
    this.scheduleDiagnostic();
  }

  flush(): void {
    this.cancelScheduled?.(); this.cancelScheduled = undefined;
    const record = this.pending; this.pending = null;
    if (record === null) return;
    try {
      this.storage().setItem(LOCAL_ALBUM_THUMBNAILS_KEY, record);
      this.stats.writes++; this.stats.stored = this.images.size;
    } catch {
      this.stats.write_failures++;
      try { this.storage().removeItem(LOCAL_ALBUM_THUMBNAILS_KEY); } catch { /* Optional cache. */ }
    }
    this.scheduleDiagnostic();
  }

  private reject(reason: RejectReason): void {
    this.stats.rejected[reason] = (this.stats.rejected[reason] ?? 0) + 1;
    this.scheduleDiagnostic();
  }

  getDiagnostics(stage: ThumbnailDiagnostic["stage"] = "summary"): ThumbnailDiagnostic {
    return { stage, ...this.stats, retained: this.images.size, rejected: { ...this.stats.rejected } };
  }

  private report(stage: ThumbnailDiagnostic["stage"]): void {
    if (!this.emit || (stage !== "final" && this.reports >= 5) || (stage === "final" && this.finalReported)) return;
    if (stage === "final") this.finalReported = true; else this.reports++;
    try { this.emit(this.getDiagnostics(stage)); } catch { /* DEV observations never affect the cache. */ }
  }

  private scheduleDiagnostic(): void {
    if (!this.emit || this.cancelDiagnostic || this.reports >= 5) return;
    const generation = this.generation;
    this.cancelDiagnostic = this.schedule(() => {
      if (generation !== this.generation) return;
      this.cancelDiagnostic = undefined; this.report("summary");
    });
  }

  finishDiagnostics(): void {
    this.flush(); this.cancelDiagnostic?.(); this.cancelDiagnostic = undefined; this.report("final");
  }

  dispose(): void {
    this.cancelScheduled?.(); this.cancelScheduled = undefined; this.pending = null;
    this.cancelDiagnostic?.(); this.cancelDiagnostic = undefined;
    this.generation++; this.folders = null; this.context = null; this.images.clear(); this.ranks.clear();
    this.observedInitial.clear(); this.capturedImages = new WeakMap();
  }
}
