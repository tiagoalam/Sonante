import type { LocalAlbum } from "../types/local";

type Entry = { started: boolean; thumb: string | null };

// One queue consumer, two outstanding reads including cache-only fallback. Old
// IPC cannot be aborted, so it keeps its slot until settled even after invalidation.
export class VisibleQueueArtwork {
  private session: object | null = null;
  private visible = new Set<LocalAlbum>();
  private entries = new Map<LocalAlbum, Entry>();
  private subscriptions = new Map<LocalAlbum, () => void>();
  private running = 0;

  private readonly load: (album: LocalAlbum, active: () => boolean) => Promise<string | null>;
  private readonly subscribe: (album: LocalAlbum, publish: (cover: string) => void) => () => void;
  private readonly changed: () => void;

  constructor(
    load: (album: LocalAlbum, active: () => boolean) => Promise<string | null>,
    subscribe: (album: LocalAlbum, publish: (cover: string) => void) => () => void,
    changed: () => void,
  ) {
    this.load = load;
    this.subscribe = subscribe;
    this.changed = changed;
  }

  begin(session: object | null): void {
    this.subscriptions.forEach(unsubscribe => unsubscribe());
    this.subscriptions.clear();
    this.session = session;
    this.visible.clear();
    this.entries.clear();
  }

  get(session: object | null, album: LocalAlbum | null): string | null {
    return session && session === this.session && album ? this.entries.get(album)?.thumb ?? null : null;
  }

  show(albums: readonly LocalAlbum[]): void {
    if (!this.session) return;
    this.visible = new Set(albums);
    for (const [album, unsubscribe] of this.subscriptions) {
      if (!this.visible.has(album)) {
        unsubscribe();
        this.subscriptions.delete(album);
        // Discard pending work which left the viewport. A return visit can retry.
        if (!this.entries.get(album)?.started || this.entries.get(album)?.thumb === null) this.entries.delete(album);
      }
    }
    const session = this.session;
    for (const album of this.visible) {
      if (!this.entries.has(album)) this.entries.set(album, { started: false, thumb: null });
      if (!this.subscriptions.has(album)) {
        this.subscriptions.set(album, this.subscribe(album, thumb => {
          if (session !== this.session || !this.visible.has(album)) return;
          const entry = this.entries.get(album);
          if (entry) { entry.thumb = thumb; this.changed(); }
        }));
      }
    }
    // Bound retained presentation images/misses. No scan or image load for offscreen rows.
    for (const album of this.entries.keys()) {
      if (this.entries.size <= 150) break;
      if (!this.visible.has(album)) this.entries.delete(album);
    }
    this.pump();
  }

  private pump(): void {
    if (!this.session) return;
    for (const album of this.visible) {
      if (this.running >= 2) break;
      const entry = this.entries.get(album);
      if (!entry || entry.started) continue;
      const session = this.session;
      const active = () => session === this.session && this.visible.has(album) && this.entries.get(album) === entry;
      entry.started = true;
      this.running++;
      void this.load(album, active).then(thumb => {
        if (active() && thumb) { entry.thumb = thumb; this.changed(); }
      }).finally(() => { this.running--; this.pump(); });
    }
  }
}
