import type { LocalAlbum } from "../types/local";

type CoverLookup = (album: LocalAlbum) => Promise<string | null>;
type FolderLookup = (path: string) => Promise<string | null>;
export type OnlineCacheStatus = "positive" | "negative" | "miss" | "ineligible";
export type ArtworkPhase = "online_request" | "online_cache_positive" | "online_cache_negative" | "online_ineligible" | "deferred";
export type CurrentFlag = boolean | (() => boolean);
const current = (flag: CurrentFlag): boolean => typeof flag === "function" ? flag() : flag;

const onlineInFlight = new Map<string, Promise<string | null>>();

export function artworkReadBlocked(updating: boolean, catalog: {
  loaded: boolean; loading: boolean; saveBlocked: boolean; mpdBusy: boolean; error: string | null;
}): boolean {
  return updating || !catalog.loaded || catalog.loading || catalog.saveBlocked || catalog.mpdBusy || !!catalog.error;
}

export class LocalArtworkResolver {
  private readonly getLocalCover: FolderLookup;
  private readonly getOnlineCover: CoverLookup;
  private readonly getOnlineCacheStatus?: (album: LocalAlbum) => Promise<OnlineCacheStatus>;
  private readonly maxCached: number;
  private readonly covers = new Map<string, string>();
  private readonly coverSources = new Map<string, "local" | "online">();
  private readonly inFlight = new Map<string, { generation: number; promise: Promise<string | null>; getPhase: () => ArtworkPhase | undefined; consumers: Set<() => boolean> }>();
  private generation = 0;
  private blocked = false;
  private catalog: readonly LocalAlbum[] | null = null;
  private active = 0;
  private readonly queued: Array<() => void> = [];
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

  getDiagnostics(): { active: number; queued: number; inFlight: number; blocked: boolean } {
    return { active: this.active, queued: this.queued.length, inFlight: this.inFlight.size, blocked: this.blocked };
  }

  setContext(blocked: boolean, catalog: readonly LocalAlbum[]): void {
    if (this.blocked === blocked && this.catalog === catalog) return;
    this.blocked = blocked;
    this.catalog = catalog;
    this.generation += 1;
  }

  private schedule(task: () => Promise<string | null>): Promise<string | null> {
    return new Promise((resolve, reject) => {
      const start = () => {
        this.active += 1;
        void task().then(resolve, reject).finally(() => {
          this.active -= 1;
          this.queued.shift()?.();
        });
      };
      if (this.active < 2) start();
      else this.queued.push(start);
    });
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

  resolve(album: LocalAlbum, onlineEnabled: CurrentFlag, libraryUpdating: CurrentFlag, onPhase?: (phase: ArtworkPhase) => void, consumerActive: CurrentFlag = true): Promise<string | null> {
    const cached = this.get(album.id);
    if (cached) return Promise.resolve(cached);
    const generation = this.generation;
    const contextBlocked = () => this.blocked || generation !== this.generation || current(libraryUpdating)
      || (this.catalog !== null && !this.catalog.includes(album));
    if (contextBlocked() || !current(consumerActive)) {
      onPhase?.("deferred");
      return Promise.resolve(null);
    }
    const interested = () => current(consumerActive);
    const observe = (entry: NonNullable<ReturnType<typeof this.inFlight.get>>) => {
      entry.consumers.add(interested);
      return entry.promise.then(cover => {
        if (!interested()) { onPhase?.("deferred"); return null; }
        if (contextBlocked() || entry.getPhase() === "deferred") { onPhase?.("deferred"); return this.get(album.id); }
        return cover;
      }).finally(() => entry.consumers.delete(interested));
    };
    const inFlight = this.inFlight.get(album.id);
    if (inFlight?.generation === generation && inFlight.getPhase() !== "deferred") return observe(inFlight);
    const consumers = new Set([interested]);
    const blocked = () => contextBlocked() || !Array.from(consumers).some(consumer => consumer());
    let phase: ArtworkPhase | undefined;
    const pending = this.schedule(() => {
      // A queued card may have disappeared, or its cover may have arrived through another path.
      if (!blocked()) {
        const cached = this.get(album.id);
        if (cached) return Promise.resolve(cached);
      }
      return lookupAlbumArtwork(album, this.getLocalCover, this.getOnlineCover, onlineEnabled, blocked, this.getOnlineCacheStatus, (nextPhase) => {
        phase = nextPhase;
        onPhase?.(nextPhase);
      });
    })
      .then((cover) => {
        if (blocked()) { phase = "deferred"; onPhase?.(phase); return this.get(album.id); }
        if (cover) this.publish(album.id, cover, phase === "online_request" || phase === "online_cache_positive" ? "online" : "local");
        return this.get(album.id) ?? cover;
      })
      .catch(error => {
        if (!blocked()) throw error;
        phase = "deferred"; onPhase?.(phase); return this.get(album.id);
      })
      .finally(() => { if (this.inFlight.get(album.id)?.promise === pending) this.inFlight.delete(album.id); });
    const entry = { generation, promise: pending, getPhase: () => phase, consumers };
    this.inFlight.set(album.id, entry);
    return observe(entry);
  }

  // Playback must never wait for an online request already in flight.
  async resolveLocalOnly(album: LocalAlbum, blocked: CurrentFlag = false): Promise<string | null> {
    const cached = this.get(album.id);
    if (cached) return cached;
    if (current(blocked)) return null;
    const cover = await lookupAlbumArtwork(album, this.getLocalCover, this.getOnlineCover, false, blocked);
    if (current(blocked)) return null;
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
  const deferred = () => { onPhase?.("deferred"); return null; };
  if (current(libraryUpdating)) return deferred();
  const paths = new Set([album.folder_path, ...album.discs.map((disc) => disc.folder_path)]);
  for (const path of paths) {
    if (current(libraryUpdating)) return deferred();
    const cover = await getLocalCover(path);
    if (current(libraryUpdating)) return deferred();
    if (cover) return cover;
  }
  if (current(libraryUpdating)) return deferred();
  if (!current(onlineEnabled)) return null;

  if (getOnlineCacheStatus) {
    const status = await getOnlineCacheStatus(album);
    if (current(libraryUpdating)) return deferred();
    if (!current(onlineEnabled)) return null;
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
  const cover = await pending;
  return current(libraryUpdating) ? deferred() : cover;
}
