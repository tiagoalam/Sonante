import type { LocalAlbum } from "../types/local";

type CoverLookup = (album: LocalAlbum) => Promise<string | null>;
type FolderLookup = (path: string) => Promise<string | null>;
export type OnlineCacheStatus = "positive" | "negative" | "miss" | "ineligible";
export type ArtworkPhase = "online_request" | "online_cache_positive" | "online_cache_negative" | "online_ineligible";
export type CurrentFlag = boolean | (() => boolean);
const current = (flag: CurrentFlag): boolean => typeof flag === "function" ? flag() : flag;

const onlineInFlight = new Map<string, Promise<string | null>>();

export class LocalArtworkResolver {
  private readonly getLocalCover: FolderLookup;
  private readonly getOnlineCover: CoverLookup;
  private readonly getOnlineCacheStatus?: (album: LocalAlbum) => Promise<OnlineCacheStatus>;
  private readonly maxCached: number;
  private readonly covers = new Map<string, string>();
  private readonly coverSources = new Map<string, "local" | "online">();
  private readonly inFlight = new Map<string, Promise<string | null>>();
  private readonly listeners = new Map<string, Set<(cover: string) => void>>();

  constructor(
    getLocalCover: FolderLookup,
    getOnlineCover: CoverLookup,
    maxCached = 150,
    getOnlineCacheStatus?: (album: LocalAlbum) => Promise<OnlineCacheStatus>,
  ) {
    this.getLocalCover = getLocalCover;
    this.getOnlineCover = getOnlineCover;
    this.maxCached = maxCached;
    this.getOnlineCacheStatus = getOnlineCacheStatus;
  }

  get(albumId: string): string | null {
    const cover = this.covers.get(albumId);
    if (!cover) return null;
    this.covers.delete(albumId);
    this.covers.set(albumId, cover);
    return cover;
  }

  getSource(albumId: string): "local" | "online" | null {
    return this.coverSources.get(albumId) ?? null;
  }

  subscribe(albumId: string, listener: (cover: string) => void): () => void {
    const listeners = this.listeners.get(albumId) ?? new Set();
    listeners.add(listener);
    this.listeners.set(albumId, listeners);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) this.listeners.delete(albumId);
    };
  }

  private publish(albumId: string, cover: string, source: "local" | "online"): void {
    if (this.covers.has(albumId)) return;
    if (this.covers.size >= this.maxCached) {
      const oldest = this.covers.keys().next().value;
      if (oldest) {
        this.covers.delete(oldest);
        this.coverSources.delete(oldest);
      }
    }
    this.covers.set(albumId, cover);
    this.coverSources.set(albumId, source);
    this.listeners.get(albumId)?.forEach((listener) => listener(cover));
  }

  resolve(album: LocalAlbum, onlineEnabled: CurrentFlag, libraryUpdating: CurrentFlag, onPhase?: (phase: ArtworkPhase) => void): Promise<string | null> {
    const cached = this.get(album.id);
    if (cached) return Promise.resolve(cached);
    const inFlight = this.inFlight.get(album.id);
    if (inFlight) return inFlight;
    let phase: ArtworkPhase | undefined;
    const pending = lookupAlbumArtwork(album, this.getLocalCover, this.getOnlineCover, onlineEnabled, libraryUpdating, this.getOnlineCacheStatus, (nextPhase) => {
      phase = nextPhase;
      onPhase?.(nextPhase);
    })
      .then((cover) => {
        if (cover) this.publish(album.id, cover, phase === "online_request" || phase === "online_cache_positive" ? "online" : "local");
        return this.get(album.id) ?? cover;
      })
      .finally(() => this.inFlight.delete(album.id));
    this.inFlight.set(album.id, pending);
    return pending;
  }

  // Playback must never wait for an online request already in flight.
  async resolveLocalOnly(album: LocalAlbum): Promise<string | null> {
    const cached = this.get(album.id);
    if (cached) return cached;
    const cover = await lookupAlbumArtwork(album, this.getLocalCover, this.getOnlineCover, false, false);
    if (cover) this.publish(album.id, cover, "local");
    return this.get(album.id) ?? cover;
  }
}

export async function lookupAlbumArtwork(
  album: LocalAlbum,
  getLocalCover: FolderLookup,
  getOnlineCover: CoverLookup,
  onlineEnabled: CurrentFlag,
  libraryUpdating: CurrentFlag,
  getOnlineCacheStatus?: (album: LocalAlbum) => Promise<OnlineCacheStatus>,
  onPhase?: (phase: ArtworkPhase) => void,
): Promise<string | null> {
  const paths = new Set([album.folder_path, ...album.discs.map((disc) => disc.folder_path)]);
  for (const path of paths) {
    const cover = await getLocalCover(path);
    if (cover) return cover;
  }
  if (!current(onlineEnabled) || current(libraryUpdating)) return null;

  if (getOnlineCacheStatus) {
    const status = await getOnlineCacheStatus(album);
    if (!current(onlineEnabled) || current(libraryUpdating)) return null;
    if (status === "negative" || status === "ineligible") {
      onPhase?.(status === "negative" ? "online_cache_negative" : "online_ineligible");
      return null;
    }
    onPhase?.(status === "positive" ? "online_cache_positive" : "online_request");
  } else {
    onPhase?.("online_request");
  }

  let pending = onlineInFlight.get(album.id);
  if (!pending) {
    pending = getOnlineCover(album).finally(() => onlineInFlight.delete(album.id));
    onlineInFlight.set(album.id, pending);
  }
  return pending;
}
