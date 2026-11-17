type Entry = { started: boolean; albumId: string | null };

// Resolve only visible opaque SQLite queue locators. Album identity comes from
// the catalog's exact locator lookup; queue presentation never guesses by tags.
export class VisibleSqliteQueueAlbums {
  private session: object | null = null;
  private visible = new Set<string>();
  private entries = new Map<string, Entry>();
  private running = 0;
  private readonly resolve: (locator: string) => Promise<string | null>;
  private readonly changed: () => void;

  constructor(resolve: (locator: string) => Promise<string | null>, changed: () => void) {
    this.resolve = resolve;
    this.changed = changed;
  }

  begin(session: object | null): void {
    this.session = session;
    this.visible.clear();
    this.entries.clear();
  }

  get(session: object | null, locator: string): string | null {
    return session && session === this.session ? this.entries.get(locator)?.albumId ?? null : null;
  }

  show(locators: readonly string[]): void {
    if (!this.session) return;
    this.visible = new Set(locators);
    for (const locator of this.entries.keys()) {
      if (!this.visible.has(locator) && this.entries.get(locator)?.albumId === null) this.entries.delete(locator);
    }
    for (const locator of this.visible) {
      if (!this.entries.has(locator)) this.entries.set(locator, { started: false, albumId: null });
    }
    for (const locator of this.entries.keys()) {
      if (this.entries.size <= 150) break;
      if (!this.visible.has(locator)) this.entries.delete(locator);
    }
    this.pump();
  }

  private pump(): void {
    if (!this.session) return;
    for (const locator of this.visible) {
      if (this.running >= 2) break;
      const entry = this.entries.get(locator);
      if (!entry || entry.started) continue;
      const session = this.session;
      const active = () => session === this.session && this.visible.has(locator) && this.entries.get(locator) === entry;
      entry.started = true;
      this.running++;
      void Promise.resolve().then(() => active() ? this.resolve(locator) : null).then(albumId => {
        if (active()) {
          entry.albumId = typeof albumId === "string" && albumId ? albumId : null;
          this.changed();
        }
      }).catch(() => {
        if (active()) this.changed();
      }).finally(() => {
        this.running--;
        this.pump();
      });
    }
  }
}
