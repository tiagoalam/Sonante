import type { LocalAlbum } from "../types/local";

export interface LocalAlbumCatalogState {
  albums: LocalAlbum[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
  mpdBusy: boolean;
  saveBlocked: boolean;
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
  if (state.albums.length > 0) return null;
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
  private inFlight = false;
  private pending = false;
  private waitingForMpd = false;
  private readonly load: () => Promise<LocalAlbum[]>;

  constructor(load: () => Promise<LocalAlbum[]>) {
    this.load = load;
  }

  getSnapshot = (): LocalAlbumCatalogState => this.state;

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
    if (this.waitingForMpd) this.setUpdating(updating);
  }

  setSaveBlocked(blocked: boolean): void {
    if (this.state.saveBlocked === blocked) return;
    this.generation += 1;
    this.publish({ saveBlocked: blocked });
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
    if (!this.pending || this.updating || this.state.saveBlocked || this.inFlight) return;
    this.pending = false;
    this.inFlight = true;
    this.waitingForMpd = false;
    const generation = ++this.generation;
    this.publish({ loading: true, error: null, mpdBusy: false });
    Promise.resolve()
      .then(() => generation === this.generation && !this.updating && !this.state.saveBlocked ? this.load() : null)
      .then((albums) => {
        if (albums === null || generation !== this.generation || this.updating) return;
        this.publish({ albums, loaded: true, error: null });
      })
      .catch((error: unknown) => {
        if (generation !== this.generation || this.updating) return;
        const message = error instanceof Error ? error.message : String(error);
        if (message === MPD_CATALOG_BUSY) {
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
