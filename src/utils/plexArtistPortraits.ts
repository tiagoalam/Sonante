import type { PlexImageRef } from "../types/plex.ts";

export type ArtistPortraitResult = { status: "ready"; image: PlexImageRef }
  | { status: "missing" | "error" | "obsolete" };
interface Entry {
  key: string; serverId: string; ratingKey: string; generation: number; users: number;
  phase: "queued" | "loading" | "settled"; result?: ArtistPortraitResult; failedAt?: number;
  promise: Promise<ArtistPortraitResult>; resolve: (result: ArtistPortraitResult) => void;
}
const obsolete: ArtistPortraitResult = { status: "obsolete" };
export const ARTIST_PORTRAIT_RETRY_MS = 30_000;

// Exact Plex identity only. Absence is cached for the session; transient errors
// may retry on a later visit after a cooldown, never through polling or renders.
export class PlexArtistPortraitCache {
  private identity: unknown = null;
  private serverId = "";
  private generation = 0;
  private active = 0;
  private entries = new Map<string, Entry>(); // insertion order is bounded LRU
  private queue: Entry[] = [];
  private load: (serverId: string, ratingKey: string) => Promise<PlexImageRef | null>;
  private now: () => number;
  private maximum: number;
  constructor(load: (serverId: string, ratingKey: string) => Promise<PlexImageRef | null>, now = Date.now, maximum = 32) {
    this.load = load; this.now = now; this.maximum = maximum;
  }

  isCurrent(identity: unknown, serverId: string) {
    return identity !== null && identity === this.identity && serverId === this.serverId;
  }
  setContext(identity: unknown, serverId: string) {
    if (this.isCurrent(identity, serverId)) return;
    this.invalidate(); this.identity = identity; this.serverId = serverId;
  }
  invalidate(identity = this.identity) {
    if (identity !== this.identity) return;
    this.generation++; this.identity = null; this.serverId = "";
    for (const entry of this.entries.values()) entry.resolve(obsolete);
    this.entries.clear(); this.queue = [];
    // Old IPC retains its concurrency slot until it actually settles.
  }
  peek(identity: unknown, serverId: string, ratingKey: string): ArtistPortraitResult | null {
    return this.isCurrent(identity, serverId) ? this.entries.get(JSON.stringify([serverId, ratingKey]))?.result ?? null : null;
  }
  acquire(identity: unknown, serverId: string, ratingKey: string) {
    const ignored = () => ({ promise: Promise.resolve(obsolete), release() {} });
    if (!this.isCurrent(identity, serverId) || !serverId || !/^[a-zA-Z0-9_-]{1,128}$/.test(ratingKey)) return ignored();
    const key = JSON.stringify([serverId, ratingKey]);
    let entry = this.entries.get(key);
    if (entry?.result?.status === "error" && this.now() - entry.failedAt! >= ARTIST_PORTRAIT_RETRY_MS) {
      this.entries.delete(key); entry = undefined;
    }
    if (!entry) {
      if (this.entries.size >= this.maximum) {
        const victim = [...this.entries.values()].find(item => item.phase === "settled" && item.users === 0);
        if (!victim) return ignored();
        this.entries.delete(victim.key);
      }
      let resolve!: Entry["resolve"];
      const promise = new Promise<ArtistPortraitResult>(done => { resolve = done; });
      entry = { key, serverId, ratingKey, generation: this.generation, users: 0, phase: "queued", promise, resolve };
      this.entries.set(key, entry); this.queue.push(entry);
    }
    const retained = entry;
    this.entries.delete(key); this.entries.set(key, entry); entry.users++;
    this.pump();
    let released = false;
    return { promise: entry.promise, release: () => {
      if (released) return; released = true; retained.users--;
      if (retained.users === 0 && retained.phase === "queued") {
        this.queue = this.queue.filter(item => item !== retained);
        if (this.entries.get(key) === retained) this.entries.delete(key);
        retained.resolve(obsolete);
      }
    } };
  }
  private pump() {
    while (this.active < 2 && this.queue.length) {
      const entry = this.queue.shift()!;
      if (entry.generation !== this.generation) continue;
      entry.phase = "loading"; this.active++;
      void Promise.resolve().then(() => entry.generation === this.generation
        ? this.load(entry.serverId, entry.ratingKey) : null).then(image => {
        if (entry.generation !== this.generation) return;
        if (image && image.server_id !== entry.serverId) throw new Error("Invalid portrait context");
        entry.result = image ? { status: "ready", image } : { status: "missing" };
      }).catch(() => {
        if (entry.generation !== this.generation) return;
        entry.result = { status: "error" }; entry.failedAt = this.now();
      }).finally(() => {
        this.active--;
        if (entry.generation === this.generation) { entry.phase = "settled"; entry.resolve(entry.result!); }
        this.pump();
      });
    }
  }
}

export function artistPortraitInitials(name: string): string | null {
  const words = name.normalize("NFC").trim().split(/\s+/u).filter(word => /[\p{L}\p{N}]/u.test(word));
  if (!words.length) return null;
  return [words[0], ...(words.length > 1 ? [words[words.length - 1]] : [])]
    .map(word => Array.from(word).find(char => /[\p{L}\p{N}]/u.test(char))!).join("").toLocaleUpperCase();
}
