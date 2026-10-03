import type { PlexImageRef } from "../types/plex";

type CacheEntry = {
  promise: Promise<string>;
  objectUrl?: string;
  byteLength: number;
  references: number;
  lastUsed: number;
  invalidated: boolean;
};

type ImageLoader = (image: PlexImageRef) => Promise<ArrayBuffer>;
type ObjectUrlFactory = (bytes: ArrayBuffer) => string;
type ObjectUrlRevoker = (url: string) => void;

export type PlexArtworkLease = {
  promise: Promise<string>;
  release: () => void;
  invalidate: () => void;
};

export const MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS = 8;

class ArtworkRequestQueue {
  private active = 0;
  private readonly waiting: Array<() => void> = [];
  private readonly maximum: number;

  constructor(maximum: number) {
    this.maximum = maximum;
  }

  run<T>(task: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const start = () => {
        this.active += 1;
        void Promise.resolve()
          .then(task)
          .then(resolve, reject)
          .finally(() => {
            this.active -= 1;
            this.waiting.shift()?.();
          });
      };

      if (this.active < this.maximum) {
        start();
      } else {
        this.waiting.push(start);
      }
    });
  }
}

export const plexImageCacheKey = (image: PlexImageRef): string =>
  `${image.server_id}\u0000${image.path}`;

export const plexCollectionCacheKey = (serverId: string, ratingKey: string): string =>
  `${serverId}\u0000${ratingKey}`;

export class PlexImageVisibilityGate {
  private imageKey: string | null = null;
  private visible = false;

  reset(image: PlexImageRef): void {
    const nextKey = plexImageCacheKey(image);
    if (this.imageKey === nextKey) return;
    this.imageKey = nextKey;
    this.visible = false;
  }

  markVisible(image: PlexImageRef): boolean {
    this.reset(image);
    if (this.visible) return false;
    this.visible = true;
    return true;
  }

  canAcquire(image: PlexImageRef): boolean {
    return this.imageKey === plexImageCacheKey(image) && this.visible;
  }
}

export class PlexArtworkCache {
  private readonly entries = new Map<string, CacheEntry>();
  private readonly loader: ImageLoader;
  private readonly createObjectUrl: ObjectUrlFactory;
  private readonly revokeObjectUrl: ObjectUrlRevoker;
  private readonly maxEntries: number;
  private readonly maxBytes: number;
  private readonly requestQueue: ArtworkRequestQueue;
  private totalBytes = 0;
  private clock = 0;

  constructor(
    loader: ImageLoader,
    createObjectUrl: ObjectUrlFactory,
    revokeObjectUrl: ObjectUrlRevoker,
    maxEntries = 96,
    maxBytes = 64 * 1024 * 1024,
    maximumConcurrentRequests = MAX_CONCURRENT_PLEX_ARTWORK_REQUESTS,
  ) {
    this.loader = loader;
    this.createObjectUrl = createObjectUrl;
    this.revokeObjectUrl = revokeObjectUrl;
    this.maxEntries = maxEntries;
    this.maxBytes = maxBytes;
    this.requestQueue = new ArtworkRequestQueue(maximumConcurrentRequests);
  }

  acquire(image: PlexImageRef): PlexArtworkLease {
    const key = plexImageCacheKey(image);
    let entry = this.entries.get(key);
    if (!entry) {
      entry = {
        promise: Promise.resolve(""),
        byteLength: 0,
        references: 0,
        lastUsed: ++this.clock,
        invalidated: false,
      };
      const createdEntry = entry;
      createdEntry.promise = this.requestQueue.run(() => this.loader(image))
        .then((bytes) => {
          const objectUrl = this.createObjectUrl(bytes);
          createdEntry.objectUrl = objectUrl;
          createdEntry.byteLength = bytes.byteLength;
          this.totalBytes += bytes.byteLength;
          this.evictIfNeeded();
          return objectUrl;
        })
        .catch((error) => {
          if (this.entries.get(key) === createdEntry) {
            this.entries.delete(key);
          }
          throw error;
        });
      this.entries.set(key, createdEntry);
    }

    entry.references += 1;
    entry.lastUsed = ++this.clock;
    let released = false;
    const release = () => {
      if (released) return;
      released = true;
      entry.references = Math.max(0, entry.references - 1);
      entry.lastUsed = ++this.clock;
      if (entry.invalidated && entry.references === 0) {
        this.revoke(entry);
      } else {
        this.evictIfNeeded();
      }
    };
    return {
      promise: entry.promise,
      release,
      invalidate: () => {
        entry.invalidated = true;
        if (this.entries.get(key) === entry) {
          this.entries.delete(key);
        }
        release();
      },
    };
  }

  clearUnused(): void {
    for (const [key, entry] of this.entries) {
      if (entry.references === 0 && entry.objectUrl) {
        this.remove(key, entry);
      }
    }
  }

  private evictIfNeeded(): void {
    while (this.entries.size > this.maxEntries || this.totalBytes > this.maxBytes) {
      let candidate: [string, CacheEntry] | undefined;
      for (const item of this.entries) {
        const entry = item[1];
        if (
          entry.references === 0 &&
          entry.objectUrl &&
          (!candidate || entry.lastUsed < candidate[1].lastUsed)
        ) {
          candidate = item;
        }
      }
      if (!candidate) return;
      this.remove(candidate[0], candidate[1]);
    }
  }

  private remove(key: string, entry: CacheEntry): void {
    if (this.entries.get(key) !== entry || !entry.objectUrl) return;
    this.entries.delete(key);
    this.revoke(entry);
  }

  private revoke(entry: CacheEntry): void {
    if (!entry.objectUrl) return;
    this.totalBytes = Math.max(0, this.totalBytes - entry.byteLength);
    this.revokeObjectUrl(entry.objectUrl);
    entry.objectUrl = undefined;
    entry.byteLength = 0;
  }
}
