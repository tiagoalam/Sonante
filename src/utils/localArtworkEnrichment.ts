import type { LocalAlbum } from "../types/local";
import type { ArtworkPhase } from "./localArtwork";
import type { ArtworkProgressChange, ArtworkProgressEntry, ArtworkProgressResult } from "../services/artworkProgress";

export interface ArtworkEnrichmentStatus {
  active: boolean;
  total: number;
  processed: number;
  found: number;
  latestFoundTitle?: string;
  onlineActivity?: { kind: "searching" | "found"; title: string };
}

export interface ArtworkResolver {
  get(albumId: string): string | null;
  getSource?(albumId: string): "local" | "online" | null;
  resolve(album: LocalAlbum, onlineEnabled: () => boolean, libraryUpdating: () => boolean, onPhase?: (phase: ArtworkPhase) => void): Promise<string | null>;
}

export interface ArtworkProgressStore {
  load(): Promise<Record<string, ArtworkProgressEntry>>;
  save(changes: ArtworkProgressChange[]): Promise<void>;
}

const yieldToBrowser = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

export class LocalArtworkEnrichment {
  private readonly resolver: ArtworkResolver;
  private readonly yieldBetweenItems: () => Promise<void>;
  private readonly reportError: (error: unknown) => void;
  private readonly progressStore?: ArtworkProgressStore;
  private persisted: Map<string, ArtworkProgressEntry> | null = null;
  private progressLoading: Promise<Map<string, ArtworkProgressEntry>> | null = null;
  private dirty: ArtworkProgressChange[] = [];
  private flushing = Promise.resolve();
  private activityTimer: ReturnType<typeof setTimeout> | null = null;
  private albums: LocalAlbum[] | null = null;
  private pendingCatalog: LocalAlbum[] | null = null;
  private ids = new Set<string>();
  private processed = new Set<string>();
  private foundIds = new Set<string>();
  private latestFoundId: string | null = null;
  private offlineMissing = new Set<string>();
  private index = 0;
  private generation = 0;
  private preparedGeneration = -1;
  private updating = false;
  private awaitingCatalog = false;
  private onlineEnabled = false;
  private running = false;
  private listeners = new Set<() => void>();
  private status: ArtworkEnrichmentStatus = { active: false, total: 0, processed: 0, found: 0 };

  constructor(
    resolver: ArtworkResolver,
    yieldBetweenItems: () => Promise<void> = yieldToBrowser,
    reportError: (error: unknown) => void = (error) => console.error("Falha ao enriquecer capa local:", error),
    progressStore?: ArtworkProgressStore,
  ) {
    this.resolver = resolver;
    this.yieldBetweenItems = yieldBetweenItems;
    this.reportError = reportError;
    this.progressStore = progressStore;
  }

  getSnapshot = (): ArtworkEnrichmentStatus => this.status;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private publish(patch: Partial<ArtworkEnrichmentStatus>): void {
    this.status = { ...this.status, ...patch };
    this.listeners.forEach((listener) => listener());
  }

  private clearActivity(): void {
    if (this.activityTimer) clearTimeout(this.activityTimer);
    this.activityTimer = null;
    if (this.status.onlineActivity) this.publish({ onlineActivity: undefined });
  }

  private foundOnline(title: string): void {
    this.clearActivity();
    this.publish({ onlineActivity: { kind: "found", title } });
    this.activityTimer = setTimeout(() => {
      this.activityTimer = null;
      this.publish({ onlineActivity: undefined });
    }, 3500);
  }

  private flush(): void {
    if (!this.progressStore || this.dirty.length === 0) return;
    const changes = this.dirty.splice(0);
    this.flushing = this.flushing.then(() => this.progressStore!.save(changes)).catch((error) => {
      this.reportError(error);
      this.dirty.unshift(...changes);
    });
  }

  private remember(albumId: string, result: ArtworkProgressResult, onlineComplete: boolean): void {
    if (!this.progressStore) return;
    this.persisted?.set(albumId, { result, online_complete: onlineComplete, checked_at: Math.floor(Date.now() / 1000) });
    this.dirty.push({ album_id: albumId, result, online_complete: onlineComplete });
    if (this.dirty.length >= 200) this.flush();
  }

  setUpdating(updating: boolean): void {
    if (this.updating === updating) return;
    this.updating = updating;
    if (updating) {
      this.awaitingCatalog = true;
      this.pendingCatalog = null;
      this.generation++;
      this.clearActivity();
      this.publish({ active: false });
      this.flush();
    } else if (this.pendingCatalog) {
      const catalog = this.pendingCatalog;
      this.pendingCatalog = null;
      this.setCatalog(catalog);
    }
    // On true -> false, wait for a successful, fresh catalog load.
  }

  setOnlineEnabled(enabled: boolean): void {
    if (this.onlineEnabled === enabled) return;
    this.onlineEnabled = enabled;
    if (enabled) {
      for (const id of this.offlineMissing) this.processed.delete(id);
      this.offlineMissing.clear();
      this.index = 0;
      this.publish({ processed: this.processed.size });
      this.start();
    }
  }

  setCatalog(albums: LocalAlbum[]): void {
    if (this.updating) {
      if (albums !== this.albums) this.pendingCatalog = albums;
      return;
    }
    if (this.awaitingCatalog && albums === this.albums) return;
    if (albums === this.albums && !this.awaitingCatalog) return;
    this.albums = albums;
    this.index = 0;
    this.generation++;
    const generation = this.generation;
    this.awaitingCatalog = false;
    // A browser task yields a paint after the catalog has been published.
    void this.prepareCatalog(generation);
  }

  private async prepareCatalog(generation: number): Promise<void> {
    await this.yieldBetweenItems();
    if (this.persisted === null) {
      this.progressLoading ??= (this.progressStore?.load() ?? Promise.resolve<Record<string, ArtworkProgressEntry>>({})).then((entries) => new Map<string, ArtworkProgressEntry>(Object.entries(entries))).catch((error) => {
        this.reportError(error);
        return new Map<string, ArtworkProgressEntry>();
      });
      this.persisted = await this.progressLoading;
    }
    if (generation !== this.generation || this.updating || this.awaitingCatalog || !this.albums) return;
    const persisted = this.persisted;
    if (!persisted) return;
    this.ids = new Set(this.albums.map((album) => album.id));
    this.processed = new Set([...this.processed].filter((id) => this.ids.has(id)));
    this.foundIds = new Set([...this.foundIds].filter((id) => this.ids.has(id)));
    if (this.latestFoundId && !this.ids.has(this.latestFoundId)) this.latestFoundId = null;
    this.offlineMissing = new Set([...this.offlineMissing].filter((id) => this.ids.has(id)));
    for (const album of this.albums) {
      const entry = persisted.get(album.id);
      if (entry && (["local", "embedded", "ineligible"].includes(entry.result)
          || entry.online_complete || (entry.result === "none" && !this.onlineEnabled))) {
        this.processed.add(album.id);
        if (entry.result === "none" && !entry.online_complete) this.offlineMissing.add(album.id);
      } else if (this.resolver.get(album.id)) {
        this.processed.add(album.id);
        const source = this.resolver.getSource?.(album.id) ?? "local";
        this.remember(album.id, source, source === "online");
      }
    }
    this.publish({
      total: this.ids.size,
      processed: this.processed.size,
      found: this.foundIds.size,
      latestFoundTitle: this.albums.find((album) => album.id === this.latestFoundId)?.title,
    });
    this.flush();
    this.preparedGeneration = generation;
    this.start();
  }

  private next(): LocalAlbum | null {
    if (!this.albums) return null;
    while (this.index < this.albums.length) {
      const album = this.albums[this.index++];
      if (!this.processed.has(album.id)) return album;
    }
    return null;
  }

  private start(): void {
    if (this.running || this.updating || this.awaitingCatalog || this.preparedGeneration !== this.generation || !this.albums?.length) return;
    if (this.index >= this.albums.length) return;
    this.running = true;
    this.publish({ active: true });
    void this.drain().finally(async () => {
      this.flush();
      await this.flushing;
      this.running = false;
      this.publish({ active: false });
      if (!this.updating && !this.awaitingCatalog && this.index < (this.albums?.length ?? 0)) this.start();
    }).catch(this.reportError);
  }

  private async drain(): Promise<void> {
    while (!this.updating && !this.awaitingCatalog) {
      const album = this.next();
      if (!album) return;
      const generation = this.generation;
      const onlineEnabledAtStart = this.onlineEnabled;
      const knownCover = this.resolver.get(album.id);
      const knownBefore = !!knownCover;
      let cover: string | null = knownCover;
      let phase: ArtworkPhase | undefined;
      let failed = false;
      try {
        cover = knownCover ?? await this.resolver.resolve(
          album, () => this.onlineEnabled, () => this.updating, (nextPhase) => {
            phase = nextPhase;
            if (nextPhase === "online_request" && generation === this.generation && !this.updating) {
              this.clearActivity();
              this.publish({ onlineActivity: { kind: "searching", title: album.title } });
            }
          },
        );
      } catch (error) {
        failed = true;
        this.reportError(error);
      }
      if (generation === this.generation && phase === "online_request" && this.status.onlineActivity?.kind === "searching"
          && this.status.onlineActivity.title === album.title) this.clearActivity();
      if (generation === this.generation && !this.updating && !this.awaitingCatalog && this.ids.has(album.id)) {
        if (!cover && !onlineEnabledAtStart && this.onlineEnabled) {
          // The preference changed while this item was resolving; retry its online phase.
          this.index = 0;
          await this.yieldBetweenItems();
          continue;
        }
        this.processed.add(album.id);
        if (!cover && !this.onlineEnabled) this.offlineMissing.add(album.id);
        if (!failed) {
          const result: ArtworkProgressResult = cover
            ? (phase === "online_request" || phase === "online_cache_positive" || this.resolver.getSource?.(album.id) === "online" ? "online" : "local")
            : phase === "online_ineligible" ? "ineligible" : "none";
          const onlineComplete = phase === "online_request" || phase === "online_cache_negative"
            || (phase === "online_cache_positive" && !!cover) || phase === "online_ineligible"
            || (!!cover && this.resolver.getSource?.(album.id) === "online");
          this.remember(album.id, result, onlineComplete);
        }
        if (cover && !knownBefore) {
          this.foundIds.add(album.id);
          this.latestFoundId = album.id;
          if (phase === "online_request") this.foundOnline(album.title);
        }
        this.publish({
          processed: this.processed.size,
          found: this.foundIds.size,
          latestFoundTitle: cover && !knownBefore ? album.title : this.status.latestFoundTitle,
        });
      }
      await this.yieldBetweenItems();
    }
  }
}
