import type { LocalAlbum } from "../types/local";
import { localAlbumPresentationContext, type LocalAlbumPresentationCache } from "./localAlbumPresentationCache.ts";

export interface LocalAlbumCatalogState {
  albums: LocalAlbum[];
  presentationAlbums?: LocalAlbum[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
  mpdBusy: boolean;
  saveBlocked: boolean;
  confirmedPlayback?: boolean;
}

// Permission is supplied by the native addition worker, never inferred from an
// updating flag. The retained list is not refreshed or promoted by this flag.
export function confirmedCatalogPlaybackAllowed(state: LocalAlbumCatalogState, updating: boolean): boolean {
  return state.loaded && !state.loading && !state.error && !state.mpdBusy
    && (state.confirmedPlayback === true || (!state.saveBlocked && !updating));
}

// Saved rows have a separate slot: existing consumers of `albums` continue to
// see only results read in this session. Only the Local browser paints these.
export function localCatalogPresentationAlbums(state: LocalAlbumCatalogState): LocalAlbum[] {
  return state.loaded ? state.albums : state.presentationAlbums ?? state.albums;
}

// Informational message selection only; native error admission is unchanged.
export function localCatalogErrorKey(error: string): "localBrowser.catalogUnsupportedFormat" | "localBrowser.catalogReadUnavailable" | null {
  if (error === "SOURCE_AVAILABILITY_UNSUPPORTED_FORMAT") return "localBrowser.catalogUnsupportedFormat";
  return /^(?:LOCAL_)?SOURCE_AVAILABILITY_(?:BUSY|BLOCKED|CHANGED|UNCONFIRMED)$/.test(error)
    ? "localBrowser.catalogReadUnavailable" : null;
}

const MPD_CATALOG_BUSY = "Biblioteca local em atualização no MPD; tente novamente após a atualização.";

export function localCatalogActivity(
  updating: boolean,
  state: Pick<LocalAlbumCatalogState, "loading" | "mpdBusy">,
): "indexing" | "organizing" | null {
  if (updating || state.mpdBusy) return "indexing";
  return state.loading ? "organizing" : null;
}

export function emptyLocalAlbumCatalogState(
  state: LocalAlbumCatalogState,
  updating: boolean,
): "indexing" | "error" | "empty" | null {
  if (localCatalogPresentationAlbums(state).length > 0) return null;
  if (updating || state.mpdBusy || state.loading || (!state.loaded && !state.error)) return "indexing";
  return state.error ? "error" : "empty";
}

export async function refreshCatalogAfterSuccessfulRescan(
  rescan: () => Promise<void>,
  notify: () => void,
): Promise<void> {
  await rescan();
  notify();
}

export async function refreshCatalogAfterSuccessfulCheck(
  check: () => Promise<boolean>,
  notify: () => void,
): Promise<boolean> {
  const changed = await check();
  if (changed) notify();
  return changed;
}

export class LocalAlbumCatalog {
  private state: LocalAlbumCatalogState = {
    albums: [], loading: false, loaded: false, error: null, mpdBusy: false, saveBlocked: false,
  };
  private listeners = new Set<() => void>();
  private updating: boolean | null = null;
  private generation = 0;
  private confirmedRevision = 0;
  private inFlight = false;
  private pending = false;
  private waitingForMpd = false;
  private readonly load: () => Promise<LocalAlbum[]>;
  private readonly presentationCache?: LocalAlbumPresentationCache;
  private contextReady: boolean;
  private presentationContext: string | null = null;
  private configuredFolders: readonly string[] | null = null;

  constructor(load: () => Promise<LocalAlbum[]>, presentationCache?: LocalAlbumPresentationCache) {
    this.load = load;
    this.presentationCache = presentationCache;
    this.contextReady = !presentationCache;
  }

  // Called synchronously with every accepted configuration, before React exposes
  // it. Restoring rows never increments the confirmed revision or grants reads.
  setPresentationContext(folders: readonly string[]): void {
    const context = localAlbumPresentationContext(folders);
    if (this.configuredFolders !== null && folders.length === this.configuredFolders.length
      && folders.every((folder, index) => folder === this.configuredFolders![index])) return;
    this.configuredFolders = [...folders];
    const wasReady = this.contextReady;
    this.contextReady = true;
    this.presentationContext = context;
    this.generation += 1;
    let albums: LocalAlbum[] = [];
    if (context !== null) {
      try { albums = this.presentationCache?.read(context) ?? []; } catch { /* Optional cache. */ }
    }
    this.waitingForMpd = false;
    this.publish({ albums: [], presentationAlbums: albums, loaded: false, loading: false, error: null, mpdBusy: false, confirmedPlayback: false });
    this.pending = this.pending || wasReady;
    this.refresh();
  }

  getSnapshot = (): LocalAlbumCatalogState => this.state;
  getConfirmedRevision = (): number => this.confirmedRevision;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private publish(patch: Partial<LocalAlbumCatalogState>): void {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((listener) => listener());
  }

  setUpdating(updating: boolean, mounted = false): void {
    const previous = this.updating;
    this.updating = updating;
    if (updating) {
      if (previous !== true) {
        this.generation += 1;
        this.pending = false;
        this.publish({ loading: false });
      }
    } else if (previous === true || mounted || this.waitingForMpd) {
      this.waitingForMpd = false;
      this.pending = true;
      this.refresh();
    }
  }

  onMpdStatus(updating: boolean): void {
    this.setUpdating(updating);
  }

  setConfirmedPlayback(allowed: boolean): void {
    const confirmedPlayback = allowed && this.state.saveBlocked && this.state.loaded;
    if (this.state.confirmedPlayback !== confirmedPlayback) this.publish({ confirmedPlayback });
  }

  setSaveBlocked(blocked: boolean): void {
    if (this.state.saveBlocked === blocked) return;
    this.generation += 1;
    this.publish({ saveBlocked: blocked, confirmedPlayback: false });
    if (!blocked) this.requestRefresh();
  }

  waitForMpd(): void {
    this.generation += 1;
    this.waitingForMpd = true;
    this.publish({ error: null, mpdBusy: true });
  }

  ensureLoaded(updating: boolean, available: boolean): void {
    if (updating) { this.setUpdating(true); return; }
    if (!available) return;
    this.setUpdating(false);
    if (this.state.loaded || this.state.loading || this.state.error
      || this.state.mpdBusy || this.state.saveBlocked || this.inFlight) return;
    this.setUpdating(false, true);
  }

  requestRefresh(): void {
    this.generation += 1;
    this.pending = true;
    this.refresh();
  }

  private refresh(): void {
    if (!this.contextReady || !this.pending || this.updating || this.state.saveBlocked || this.inFlight) return;
    this.pending = false;
    this.inFlight = true;
    this.waitingForMpd = false;
    const generation = ++this.generation;
    this.publish({ loading: true, error: null, mpdBusy: false });
    Promise.resolve()
      .then(() => generation === this.generation && !this.updating && !this.state.saveBlocked ? this.load() : null)
      .then((albums) => {
        if (albums === null || generation !== this.generation || this.updating) return;
        this.confirmedRevision += 1;
        this.publish({ albums, presentationAlbums: undefined, loaded: true, error: null });
        // Recheck after subscribers: a configuration change must not persist an
        // old result under the new context. Busy/failed/stale reads never write.
        if (generation === this.generation && this.presentationContext !== null) {
          try { this.presentationCache?.write(this.presentationContext, albums); } catch { /* Optional cache. */ }
        }
      })
      .catch((error: unknown) => {
        if (generation !== this.generation || this.updating) return;
        const message = error instanceof Error ? error.message : String(error);
        if (message === MPD_CATALOG_BUSY || message === "SOURCE_AVAILABILITY_BUSY") {
          this.waitingForMpd = true;
          this.publish({ error: null, mpdBusy: true });
        } else {
          this.publish({ error: message });
        }
      })
      .finally(() => {
        this.inFlight = false;
        this.publish({ loading: false });
        this.refresh();
      });
  }
}
