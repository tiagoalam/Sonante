import type { PlexAlbum, PlexLibrary } from "../types/plex.ts";

export interface PlexLibraryCatalog {
  library: PlexLibrary;
  phase: "queued" | "loading" | "complete" | "error";
  albums: PlexAlbum[];
}
export interface PlexCatalogSnapshot {
  identity: unknown;
  serverId: string;
  started: boolean;
  libraries: readonly PlexLibraryCatalog[];
  albums: PlexAlbum[];
  catalogs: readonly { libraryId: string; albums: PlexAlbum[] }[];
}
interface Request {
  library: PlexLibrary;
  generation: number;
  promise: Promise<PlexAlbum[]>;
  resolve: (albums: PlexAlbum[]) => void;
  reject: (error: unknown) => void;
}

// Session memory only. Browser and Explore use this same queue and inventory.
export class PlexCatalogCoordinator {
  private state: PlexCatalogSnapshot = { identity: null, serverId: "", started: false, libraries: [], albums: [], catalogs: [] };
  private generation = 0;
  private active = 0;
  private requests = new Map<string, Request>();
  private queue: Request[] = [];
  private listeners = new Set<() => void>();
  private load: (libraryId: string) => Promise<PlexAlbum[]>;
  private reportError: (error: unknown) => void;
  constructor(load: (libraryId: string) => Promise<PlexAlbum[]>, reportError: (error: unknown) => void = () => {}) {
    this.load = load; this.reportError = reportError;
  }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };

  setContext(identity: unknown, serverId: string): void {
    if (this.state.identity === identity && this.state.serverId === serverId) return;
    this.invalidate();
    this.state = { ...this.state, identity, serverId };
    this.emit();
  }
  invalidate(identity = this.state.identity): void {
    if (identity !== this.state.identity) return;
    this.generation++;
    for (const request of this.requests.values()) request.reject(new Error("Contexto Plex encerrado."));
    this.requests.clear(); this.queue = [];
    // Running requests keep their slots until they actually finish. New contexts
    // cannot exceed the two-request budget while obsolete IPC is still settling.
    this.state = { identity: null, serverId: "", started: false, libraries: [], albums: [], catalogs: [] };
    this.emit();
  }
  private emit(): void { this.listeners.forEach(listener => listener()); }
  private publish(libraries: readonly PlexLibraryCatalog[]): void {
    const complete = libraries.filter(item => item.phase === "complete");
    const unchanged = complete.length === this.state.catalogs.length && complete.every((item, i) => item.library.key === this.state.catalogs[i].libraryId && item.albums === this.state.catalogs[i].albums);
    this.state = { ...this.state, libraries,
      catalogs: unchanged ? this.state.catalogs : complete.map(item => ({ libraryId: item.library.key, albums: item.albums })),
      albums: unchanged ? this.state.albums : complete.flatMap(item => item.albums) };
    this.emit();
  }
  loadLibrary(library: PlexLibrary, retry = false): Promise<PlexAlbum[]> {
    if (this.state.identity === null) return Promise.reject(new Error("Contexto Plex indisponível."));
    const pending = this.requests.get(library.key); if (pending) return pending.promise;
    const existing = this.state.libraries.find(item => item.library.key === library.key);
    if (existing?.phase === "complete") return Promise.resolve(existing.albums);
    if (existing?.phase === "error" && !retry) return Promise.reject(new Error("Biblioteca Plex não carregada."));
    let resolve!: Request["resolve"], reject!: Request["reject"];
    const promise = new Promise<PlexAlbum[]>((a, b) => { resolve = a; reject = b; });
    const request = { library, generation: this.generation, promise, resolve, reject };
    this.requests.set(library.key, request);
    const record: PlexLibraryCatalog = { library, phase: "queued", albums: [] };
    this.publish(existing ? this.state.libraries.map(item => item.library.key === library.key ? record : item) : [...this.state.libraries, record]);
    this.queue.push(request); this.pump(); return promise;
  }
  aggregate(libraries: readonly PlexLibrary[]): void {
    if (this.state.identity === null) return;
    if (!this.state.started) { this.state = { ...this.state, started: true }; this.emit(); }
    for (const library of libraries) void this.loadLibrary(library).catch(() => {});
  }
  retryFailures(): void {
    for (const item of this.state.libraries.filter(item => item.phase === "error")) void this.loadLibrary(item.library, true).catch(() => {});
  }
  private pump(): void {
    while (this.active < 2 && this.queue.length) {
      const request = this.queue.shift()!;
      if (request.generation !== this.generation) continue;
      this.active++;
      const serverId = this.state.serverId;
      this.publish(this.state.libraries.map(item => item.library.key === request.library.key ? { ...item, phase: "loading" } : item));
      Promise.resolve().then(() => {
        if (request.generation !== this.generation) throw new Error("Contexto Plex encerrado.");
        return this.load(request.library.key);
      }).then(albums => {
        if (request.generation !== this.generation) return;
        const scoped = albums.map(album => ({ ...album, server_id: serverId, library_id: request.library.key, library_title: request.library.title }));
        this.publish(this.state.libraries.map(item => item.library.key === request.library.key ? { ...item, phase: "complete", albums: scoped } : item));
        request.resolve(scoped);
      }).catch(error => {
        if (request.generation !== this.generation) return;
        if (this.requests.get(request.library.key) === request) this.requests.delete(request.library.key);
        this.publish(this.state.libraries.map(item => item.library.key === request.library.key ? { ...item, phase: "error" } : item));
        request.reject(error);
        try { this.reportError(error); } catch { /* Reporting cannot block retries. */ }
      }).finally(() => {
        this.active--;
        if (this.requests.get(request.library.key) === request) this.requests.delete(request.library.key);
        this.pump();
      });
    }
  }
}
