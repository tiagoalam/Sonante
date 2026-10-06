import type { LocalAlbum } from "../types/local";

export interface LocalAlbumCatalogState {
  albums: LocalAlbum[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
}

export function emptyLocalAlbumCatalogState(
  state: LocalAlbumCatalogState,
  updating: boolean,
): "indexing" | "error" | "empty" | null {
  if (state.albums.length > 0) return null;
  if (updating || state.loading || (!state.loaded && !state.error)) return "indexing";
  return state.error ? "error" : "empty";
}

export class LocalAlbumCatalog {
  private state: LocalAlbumCatalogState = {
    albums: [], loading: false, loaded: false, error: null,
  };
  private listeners = new Set<() => void>();
  private updating: boolean | null = null;
  private generation = 0;
  private inFlight = false;
  private pending = false;
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
    } else if (previous === true || mounted) {
      this.pending = true;
      this.refresh();
    }
  }

  private refresh(): void {
    if (!this.pending || this.updating || this.inFlight) return;
    this.pending = false;
    this.inFlight = true;
    const generation = ++this.generation;
    this.publish({ loading: true, error: null });
    Promise.resolve()
      .then(() => generation === this.generation && !this.updating ? this.load() : null)
      .then((albums) => {
        if (albums === null || generation !== this.generation || this.updating) return;
        this.publish({ albums, loaded: true, error: null });
      })
      .catch((error: unknown) => {
        if (generation !== this.generation || this.updating) return;
        this.publish({ error: error instanceof Error ? error.message : String(error) });
      })
      .finally(() => {
        this.inFlight = false;
        this.publish({ loading: false });
        this.refresh();
      });
  }
}
