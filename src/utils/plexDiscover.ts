import type { PlexDiscoverPage, PlexDiscoverRecent, PlexDiscoverHub, PlexDiscoverItem, PlexDiscoverArtistContent, SelectedArtist, PlexAlbum, PlexLibrary } from "../types/plex.ts";
import { usablePresentationText } from "./explorePresentation.ts";
import { discoverItemKey } from "./plexDiscoverPresentation.ts";
import type { PlexLibraryMixPage, PlexMixTrack, PlexMixYears, PlexMixStyles, PlexStyleSelection } from "../types/plex.ts";
import { MIX_LIMIT, balancedMix, mixTrackKey, shuffleMix, mixPlaylistItems } from "./plexLibraryMix.ts";
import { logPlexMixDiagnostic } from "./frontendDiagnostics.ts";
import type { NewPlaylistItem } from "../types/playlist.ts";
export interface DiscoverScope { identity: unknown; serverId: string; libraryId: string; language: "pt-BR" | "en"; libraries: PlexLibrary[] }
type Phase = "idle" | "loading" | "ready" | "error";
export interface DiscoverLibraryState {
  recentPhase: Phase; recentError: string | null; recentPartial: boolean; recentDatesMissing: boolean;
  foundHubs: number; library: PlexLibrary; phase: Phase; page: PlexDiscoverPage | null; error: string | null;
}
interface Hero { phase: Phase; candidates: PlexDiscoverItem[]; item: PlexDiscoverItem | null; selectionCount: number; selectionPartial: boolean; partial: boolean; datesMissing: boolean }
const emptyHero = (): Hero => ({ phase: "idle", candidates: [], item: null, selectionCount: 0, selectionPartial: false, partial: false, datesMissing: false });
interface YearRow { library: PlexLibrary; phase: Phase; years: number[]; partial: boolean; error: string | null }
interface StyleRow { library: PlexLibrary; phase: Phase; field: PlexStyleSelection["field"] | null; choices: PlexMixStyles["choices"]; partial: boolean; error: string | null }
const emptyStyles = () => ({ opened: false, rows: [] as StyleRow[] });
const styleKey = (title: string) => title.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase();
const emptyDecades = () => ({ opened: false, rows: [] as YearRow[] });
interface Snapshot {
  mixMode: "library" | "decade" | "style"; mixStyle: {key:string; title:string} | null; mixDecade: number | null;
  styles: ReturnType<typeof emptyStyles>;
  decades: ReturnType<typeof emptyDecades>;
  mixSave: { opened: boolean; name: string; busy: boolean; error: string | null; saved: boolean };
  mix: { opened: boolean; phase: Phase; tracks: PlexMixTrack[]; partial: boolean; limited: boolean; playing: boolean; playError: boolean; issues: { library: string; error: string }[] };
  hero: Hero;
  libraries: DiscoverLibraryState[]; hubs: PlexDiscoverHub[]; totalLibraries: number;
  detail: { item: PlexDiscoverItem; phase: Phase; hub: PlexDiscoverHub | null; error: string | null } | null;
}
const emptyMix = (): Snapshot["mix"] => ({ opened: false, phase: "idle", tracks: [], partial: false, limited: false, playing: false, playError: false, issues: [] });
const emptyMixSave = (): Snapshot["mixSave"] => ({ opened: false, name: "", busy: false, error: null, saved: false });
const empty = (): Snapshot => ({ mixMode: "library", mixStyle: null, styles: emptyStyles(), mixDecade: null, decades: emptyDecades(), mixSave: emptyMixSave(), mix: emptyMix(), hero: emptyHero(), libraries: [], hubs: [], totalLibraries: 0, detail: null });
interface MixRow { library: PlexLibrary; phase: Phase; limit: number; tracks: PlexMixTrack[]; more: boolean; partial: boolean; error: string | null }
export function discoverError(error: unknown) {
  return typeof error === "string" && ["PLEX_DISCOVER_STYLE_UNSUPPORTED", "PLEX_DISCOVER_AUTH", "PLEX_DISCOVER_CONTEXT", "PLEX_DISCOVER_INVALID", "PLEX_DISCOVER_UNAVAILABLE"].includes(error) ? error : "PLEX_DISCOVER_UNAVAILABLE";
}
export const DISCOVER_RECENT_LIMIT = 24;
export const DISCOVER_OTHER_SHELVES = 8;
export const DISCOVER_ARTIST_LIMIT = 10;
export const DISCOVER_HERO_LIMIT = 20;
// Aggregate shelf24 + up to8 legacy recent rows x12, plus20 eligible albums.
export const DISCOVER_HERO_WINDOW = DISCOVER_RECENT_LIMIT + DISCOVER_OTHER_SHELVES * 12 + DISCOVER_HERO_LIMIT;
// Two actual IPCs globally; page/recent admission shares four pending jobs,
// plus one urgent detail. Hero stores <=20
// eligible candidates + one selected item + a globally capped raw window.
// No per-library candidate arrays are retained; shelf order is unchanged.
// All mode retains <=72 recent cards (24 per distinct album/artist family) and <=96 distinct recommendation cards.
// Metadata-only progress covers every accessible library, without pagination.
type RecentItem = PlexDiscoverItem & { source_order?: number };
type RecentFamily = NonNullable<PlexDiscoverHub["recent_family"]>;
const recentDate = (item: PlexDiscoverItem, family: RecentFamily) => {
  const n = family === "added" ? item.added_at : item.last_viewed_at;
  return typeof n === "number" && Number.isSafeInteger(n) && n > 0 ? n : null;
};
export function mergeRecentItems(previous: RecentItem[], incoming: PlexDiscoverItem[], family: RecentFamily, libraries: PlexLibrary[], pinned: PlexDiscoverItem | null = null) {
  const order = new Map(libraries.map((library, index) => [library.key, index]));
  const unique = new Map<string, RecentItem>(previous.flatMap(item => family === "played_artists" ? item.artist_members ?? [item] : [item]).map(item => [discoverItemKey(item), item]));
  incoming.forEach((item, index) => unique.set(discoverItemKey(item), { ...item, source_order: index }));
  // Actual date descending; undated entries form a separately disclosed tail.
  // Ties/undated entries use stable server order and library order, not arrival.
  const sorted = [...unique.values()].sort((a, b) => {
    const x = recentDate(a, family), y = recentDate(b, family);
    if (x !== null || y !== null) {
      if (x === null) return 1; if (y === null) return -1;
      if (x !== y) return y - x;
    }
    return (a.source_order ?? 0) - (b.source_order ?? 0) || (order.get(a.library_id) ?? 0) - (order.get(b.library_id) ?? 0)
      || discoverItemKey(a).localeCompare(discoverItemKey(b));
  });
  if (family === "played_artists") {
    const groups = new Map<string, RecentItem[]>();
    for (const item of sorted) {
      const name = item.kind === "artist" ? usablePresentationText(item.title) : null;
      const key = name ? JSON.stringify(["artist", item.server_id, name]) : discoverItemKey(item);
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(item);
    }
    const cards = [...groups].map(([presentation_key, members]) => ({ ...members[0], presentation_key, artist_members: members }));
    const pin = pinned && cards.findIndex(card => card.artist_members.some(item => discoverItemKey(item) === discoverItemKey(pinned)));
    return typeof pin === "number" && pin >= DISCOVER_RECENT_LIMIT ? [...cards.slice(0, DISCOVER_RECENT_LIMIT - 1), cards[pin]] : cards.slice(0, DISCOVER_RECENT_LIMIT);
  }
  const pin = pinned && sorted.findIndex(item => discoverItemKey(item) === discoverItemKey(pinned));
  if (typeof pin === "number" && pin >= 0) sorted[pin] = pinned!;
  return typeof pin === "number" && pin >= DISCOVER_RECENT_LIMIT ? [...sorted.slice(0, DISCOVER_RECENT_LIMIT - 1), pinned!] : sorted.slice(0, DISCOVER_RECENT_LIMIT);
}
export const discoverAlbumKey = (item: PlexDiscoverItem) => JSON.stringify([item.server_id, item.library_id, item.album?.rating_key ?? item.rating_key]);
export function recentShelfAlbumKeys(hubs: PlexDiscoverHub[]) {
  const keys = new Set<string>();
  for (const hub of hubs) {
    // Use classified sort contract or the already documented built-in music
    // identity/context. Custom titles never identify an added shelf.
    const added = hub.recent_family === "added" || ((hub.source_id ?? hub.id) === "home.music.recent" && hub.context === "hub.home.music.recent");
    if (added) for (const item of hub.items) if (item.album) keys.add(discoverAlbumKey(item));
  }
  return keys;
}
export function mergeHeroCandidates(previous: PlexDiscoverItem[], incoming: PlexDiscoverItem[], all: boolean, limit = DISCOVER_HERO_LIMIT) {
  const unique = new Map(previous.map(item => [discoverItemKey(item), item]));
  for (const item of incoming) if (!all || recentDate(item, "added") !== null) unique.set(discoverItemKey(item), item);
  // Keep the global recent window before exclusion, not a fixed skip per
  // source. Its bound covers the displayed shelf plus20 eligible candidates.
  return [...unique.values()].sort((a, b) => (recentDate(b, "added") ?? 0) - (recentDate(a, "added") ?? 0)
    || discoverItemKey(a).localeCompare(discoverItemKey(b))).slice(0, limit);
}
export function artistSuggestionHubs(hubs: PlexDiscoverHub[], scope: DiscoverScope): PlexDiscoverHub[] {
  if (scope.libraryId === "all") return hubs.filter(hub => !hub.artist_recommendation).map(hub => ({ ...hub, artist_candidates: undefined }));
  const first = hubs.findIndex(hub => hub.artist_recommendation);
  if (first < 0) return hubs;
  const candidates = [...hubs.filter(hub => hub.artist_recommendation), ...hubs.filter(hub => !hub.artist_recommendation && !hub.recent_family)];
  const artists = new Map<string, PlexDiscoverItem>();
  for (const hub of candidates) for (const item of hub.artist_candidates ?? []) {
    const key = discoverItemKey(item);
    if (artists.size < DISCOVER_ARTIST_LIMIT && !artists.has(key)) artists.set(key, item);
  }
  const suggestions: PlexDiscoverHub = { id: JSON.stringify([scope.serverId, scope.libraryId, "sonante", "artist_suggestions"]), presentation: "artist_suggestions",
    library_id: scope.libraryId, title: "", items: [...artists.values()], partial: false };
  return hubs.flatMap((hub, index) => index === first ? [suggestions] : hub.artist_recommendation ? [] : [hub]);
}
export class PlexDiscoverSession {
  private mixSaveItems: NewPlaylistItem[] | null = null; private mixSaveRevision = 0; private mixSaving = false;
  private mixRun: { revision: number; round: number; rows: MixRow[] } | null = null;
  private mixRevision = 0; private mixSubmitting = false;
  private mixQuery?: (server: string, library: string, language: string, limit: number, decade?: number, style?: PlexStyleSelection) => Promise<PlexLibraryMixPage>;
  private yearsQuery?: (server: string, library: string, language: string) => Promise<PlexMixYears>;
  private stylesQuery?: (server: string, library: string, language: string) => Promise<PlexMixStyles>;
  stylesPosition = 0;
  private mixCache = new Map<Snapshot["mixMode"], { mix: Snapshot["mix"]; decade: number | null; style: Snapshot["mixStyle"]; position: number; focus: PlexDiscoverSession["mixFocus"] }>();
  decadesPosition = 0;
  private state = empty(); private scope: DiscoverScope | null = null;
  private generation = 0; private running = 0; private requested = false; private pinned: PlexDiscoverItem | null = null;
  private queue: { generation: number; kind: string; run: () => Promise<void>; cancel?: () => void }[] = [];
  private listeners = new Set<() => void>(); private details = new Map<string, PlexDiscoverHub>(); private shelves = new Map<string, number>();
  position = 0; detailPosition = 0; mixPosition = 0;
  mixFocus: "play" | "save" | "another" | "change" | null = null;
  private load: (server: string, library: string, language: string) => Promise<PlexDiscoverPage>;
  private children: (server: string, library: string, kind: string, id: string, language: string) => Promise<PlexDiscoverHub>;
  private recent: (server: string, library: string, language: string, windowSize: number) => Promise<PlexDiscoverRecent>;
  private random: () => number;
  private fullArtist?: (server: string, library: string, id: string, language: string) => Promise<PlexDiscoverArtistContent>;
  private heroRaw: PlexDiscoverItem[] = []; private recentWindow = DISCOVER_HERO_LIMIT;
  private exclusionSignature: string | null = null; private heroRevision = 0;
  constructor(load: PlexDiscoverSession["load"], children: PlexDiscoverSession["children"], recent: PlexDiscoverSession["recent"], random: () => number = Math.random, fullArtist?: PlexDiscoverSession["fullArtist"], mixQuery?: PlexDiscoverSession["mixQuery"], yearsQuery?: PlexDiscoverSession["yearsQuery"], stylesQuery?: PlexDiscoverSession["stylesQuery"]) {
    this.load = load; this.children = children; this.recent = recent; this.random = random;
    this.fullArtist = fullArtist;
    this.mixQuery = mixQuery; this.yearsQuery = yearsQuery; this.stylesQuery = stylesQuery;
  }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private addedKeys(state = this.state) { return recentShelfAlbumKeys(this.scope?.libraryId === "all" ? state.hubs : state.libraries.flatMap(row => row.page?.hubs ?? [])); }
  private shelvesSettled() { return this.state.libraries.every(row => row.phase === "ready" || row.phase === "error"); }
  private publish(patch: Partial<Snapshot>) {
    const next = { ...this.state, ...patch }, excluded = this.addedKeys(next);
    const candidates = next.hero.candidates.filter(item => !excluded.has(discoverAlbumKey(item)));
    const invalid = !!next.hero.item && excluded.has(discoverAlbumKey(next.hero.item));
    // Revalidate atomically with a shelf update, including horizontally offscreen
    // cards. No subscriber/render can observe a newly duplicated hero.
    next.hero = { ...next.hero, candidates, ...(invalid ? { item: null, selectionCount: 0, selectionPartial: false, phase: "loading" as Phase } : {}) };
    if (next.libraries.some(row => row.phase === "idle" || row.phase === "loading")) next.hero.phase = "loading";
    this.state = next; this.listeners.forEach(fn => fn());
  }
  private update(id: string, patch: Partial<DiscoverLibraryState>) { this.publish({ libraries: this.state.libraries.map(row => row.library.key === id ? { ...row, ...patch } : row) }); }
  private targets() {
    return this.scope?.libraryId === "all" ? this.scope.libraries : this.scope?.libraries.filter(l => l.key === this.scope?.libraryId) ?? [];
  }
  private reset() {
    this.mixCache.clear(); this.decadesPosition = 0; this.stylesPosition = 0;
    this.mixSaveRevision++; this.mixSaveItems = null; this.mixPosition = 0; this.mixFocus = null;
    this.mixRevision++; this.mixRun = null;
    this.queue.forEach(request => request.cancel?.());
    this.generation++; this.requested = false; this.pinned = null; this.queue = []; this.details.clear(); this.shelves.clear(); this.position = 0; this.detailPosition = 0;
    this.heroRaw = []; this.recentWindow = DISCOVER_HERO_LIMIT; this.exclusionSignature = null; this.heroRevision++;
    const targets = this.targets();
    this.state = { mixMode: "library", mixStyle: null, styles: emptyStyles(), mixDecade: null, decades: emptyDecades(), mixSave: { ...emptyMixSave(), busy: this.mixSaving }, mix: { ...emptyMix(), playing: this.mixSubmitting }, hero: emptyHero(), detail: null, hubs: [], totalLibraries: targets.length, libraries: targets.map(library => ({ recentPhase: "idle", recentError: null, recentPartial: false, recentDatesMissing: false, foundHubs: 0, library, phase: "idle", page: null, error: null })) };
    this.listeners.forEach(fn => fn());
  }
  setContext(scope: DiscoverScope | null) {
    // Membership changes matter; library labels/order and unrelated libraries
    // in individual mode are not new playback/media identities or new draws.
    const keys = (value: DiscoverScope | null) => (value?.libraries.filter(library => value.libraryId === "all" || library.key === value.libraryId) ?? []).map(library => library.key).sort();
    if (scope?.identity === this.scope?.identity && scope?.serverId === this.scope?.serverId && scope?.libraryId === this.scope?.libraryId && scope?.language === this.scope?.language
      && JSON.stringify(keys(scope)) === JSON.stringify(keys(this.scope))) return;
    this.scope = scope; this.reset();
  }
  isScope(scope: DiscoverScope) { const current = this.scope; return !!current && current.identity === scope.identity && current.serverId === scope.serverId && current.libraryId === scope.libraryId && current.language === scope.language; }
  shelfPosition(id: string) { return this.shelves.get(id) ?? 0; }
  saveShelfPosition(id: string, position: number) { this.shelves.delete(id); this.shelves.set(id, Math.max(0, position)); if (this.shelves.size > 16) this.shelves.delete(this.shelves.keys().next().value!); }
  private enqueue(kind: string, run: (scope: DiscoverScope, current: () => boolean) => Promise<void>, cancel?: () => void) {
    if (!this.scope) return;
    const scope = this.scope, generation = this.generation;
    this.queue = this.queue.filter(request => { if (request.kind === kind) { request.cancel?.(); return false; } return true; });
    const request = { kind, generation, cancel, run: () => run(scope, () => generation === this.generation && this.scope === scope) };
    // User navigation precedes queued background suggestions; active requests keep their slots.
    if (kind === "detail" || kind === "artist") this.queue.unshift(request); else this.queue.push(request);
    this.pump();
  }
  private pump() {
    while (this.running < 2 && this.queue.length) {
      const request = this.queue.shift()!; if (request.generation !== this.generation) { request.cancel?.(); continue; }
      this.running++; void request.run().finally(() => { this.running--; this.pump(); if (this.requested) this.ensure(); });
    }
  }
  ensure() {
    this.requested = true;
    this.ensureStyles();
    this.ensureYears();
    this.ensureMix();
    for (const row of this.state.libraries) {
      if (this.queue.length >= 4) break;
      if (row.phase === "idle") this.loadLibrary(row);
    }
    if (!this.shelvesSettled()) return;
    this.configureRecentWindow();
    for (const row of this.state.libraries) {
      if (this.queue.length >= 4) break;
      if (row.recentPhase === "idle") this.loadRecent(row);
    }
    this.settleHero();
  }
  private switchMix(mode: Snapshot["mixMode"]) {
    if (mode === this.state.mixMode) return true;
    if (this.mixSaving || this.mixSubmitting) return false;
    const previous = this.state.mix;
    this.mixCache.set(this.state.mixMode, { mix: previous.phase === "loading" ? emptyMix() : previous,
      decade: this.state.mixDecade, style: this.state.mixStyle, position: this.mixPosition, focus: this.mixFocus });
    this.mixRevision++; this.mixRun = null; this.queue = this.queue.filter(job => !job.kind.startsWith("mix:"));
    this.cancelMixSave();
    const cached = this.mixCache.get(mode);
    this.mixPosition = cached?.position ?? 0; this.mixFocus = cached?.focus ?? null;
    this.publish({ mixMode: mode, mixDecade: cached?.decade ?? null, mixStyle: cached?.style ?? null, mix: { ...(cached?.mix ?? emptyMix()), opened: false } });
    return true;
  }
  openStyles() {
    if (!this.scope || !this.stylesQuery || this.mixSaving || this.mixSubmitting || !this.switchMix("style")) return;
    this.closeDecades(); this.cancelMixSave();
    this.publish({ mix: { ...this.state.mix, opened: false }, styles: { opened: true,
      rows: this.state.styles.rows.length ? this.state.styles.rows : this.targets().map(library => ({ library, phase: "idle", field:null, choices:[], partial:false, error:null })) } });
    this.ensure();
  }
  closeStyles() { this.publish({ styles: { ...this.state.styles, opened:false } }); }
  styleOptions() {
    const options = new Map<string,{key:string; title:string}>();
    for (const row of this.state.styles.rows) for (const choice of row.choices) {
      const key=styleKey(choice.title); if (!options.has(key)) options.set(key,{key,title:choice.title});
    }
    return [...options.values()].sort((a,b)=>a.title.localeCompare(b.title));
  }
  chooseStyle(key: string) {
    if (this.state.mixMode!=="style" || this.mixSaving || this.mixSubmitting || this.state.styles.rows.some(row=>row.phase==="idle" || row.phase==="loading")) return;
    const choice=this.styleOptions().find(choice=>choice.key===key); if (!choice) return;
    if (this.state.mixStyle?.key!==key) {
      this.mixRevision++; this.mixRun=null; this.queue=this.queue.filter(job=>!job.kind.startsWith("mix:"));
      this.mixPosition=0;this.mixFocus=null;this.cancelMixSave();this.publish({mixStyle:choice,mix:emptyMix()});
    }
    this.closeStyles();this.openMix("style");
  }
  retryStyles() {
    this.publish({styles:{...this.state.styles,rows:this.state.styles.rows.map(row=>row.phase==="error"?{...row,phase:"idle",error:null}:row)}});this.ensure();
  }
  private styleSelection(library: string): PlexStyleSelection | undefined {
    const row=this.state.styles.rows.find(row=>row.library.key===library);
    const ids=row?.choices.filter(choice=>styleKey(choice.title)===this.state.mixStyle?.key).map(choice=>choice.id);
    return row?.field && ids?.length ? {field:row.field,ids} : undefined;
  }
  private ensureStyles() {
    if (!this.stylesQuery) return;
    for (const row of this.state.styles.rows) {
      if (this.queue.length>=4) break; if (row.phase!=="idle") continue; row.phase="loading";
      this.enqueue(`styles:${row.library.key}`,async(scope,current)=>{
        let choices:PlexMixStyles["choices"]=[],field:StyleRow["field"]=null,partial=false,error:string|null=null;
        try {
          const result=await this.stylesQuery!(scope.serverId,row.library.key,scope.language); if (!current()) return;
          if (result.server_id!==scope.serverId || result.library_id!==row.library.key || result.language!==scope.language) throw "PLEX_DISCOVER_CONTEXT";
          if (!["artist.style","album.style","track.style"].includes(result.field) || !Array.isArray(result.choices) || result.choices.length>256 || typeof result.partial!=="boolean"
            || result.choices.some(choice=>!choice || typeof choice.id!=="string" || !/^\d{1,20}$/.test(choice.id) || typeof choice.title!=="string" || !choice.title.trim() || choice.title.length>1024 || /[\x00-\x1f\x7f]/.test(choice.title))) throw "PLEX_DISCOVER_INVALID";
          if (new Set(result.choices.map(choice=>choice.id)).size!==result.choices.length) throw "PLEX_DISCOVER_INVALID";
          choices=result.choices;field=result.field;partial=result.partial;
        } catch(cause) { if (!current()) return; error=discoverError(cause); }
        this.publish({styles:{...this.state.styles,rows:this.state.styles.rows.map(r=>r===row?{...r,phase:error?"error":"ready",choices,field,partial,error}:r)}});
      });
    }
  }
  openDecades() {
    if (!this.scope || !this.yearsQuery || this.mixSaving || this.mixSubmitting || !this.switchMix("decade")) return;
    this.closeStyles(); this.cancelMixSave();
    this.publish({ mix: { ...this.state.mix, opened: false }, decades: { opened: true,
      rows: this.state.decades.rows.length ? this.state.decades.rows : this.targets().map(library => ({ library, phase: "idle", years: [], partial: false, error: null })) } });
    this.ensure();
  }
  closeDecades() { this.publish({ decades: { ...this.state.decades, opened: false } }); }
  decadeOptions() { return [...new Set(this.state.decades.rows.flatMap(row => row.years.map(year => Math.floor(year / 10) * 10)))].sort((a, b) => b - a); }
  chooseDecade(decade: number) {
    if (this.state.mixMode !== "decade" || this.state.decades.rows.some(row => row.phase === "idle" || row.phase === "loading") || this.mixSaving || this.mixSubmitting || !this.decadeOptions().includes(decade)) return;
    if (this.state.mixDecade !== decade) {
      this.mixRevision++; this.mixRun = null; this.queue = this.queue.filter(job => !job.kind.startsWith("mix:"));
      this.mixPosition = 0; this.mixFocus = null; this.cancelMixSave();
      this.publish({ mixDecade: decade, mix: emptyMix() });
    }
    this.publish({ decades: { ...this.state.decades, opened: false } }); this.openMix("decade");
  }
  retryYears() {
    this.publish({ decades: { ...this.state.decades, rows: this.state.decades.rows.map(row => row.phase === "error" ? { ...row, phase: "idle", error: null } : row) } });
    this.ensure();
  }
  private ensureYears() {
    if (!this.yearsQuery) return;
    for (const row of this.state.decades.rows) {
      if (this.queue.length >= 4) break;
      if (row.phase !== "idle") continue;
      row.phase = "loading";
      this.enqueue(`years:${row.library.key}`, async (scope, current) => {
        let years: number[] = [], partial = false, error: string | null = null;
        try {
          const result = await this.yearsQuery!(scope.serverId, row.library.key, scope.language);
          if (!current()) return;
          if (result.server_id !== scope.serverId || result.library_id !== row.library.key || result.language !== scope.language) throw "PLEX_DISCOVER_CONTEXT";
          if (!Array.isArray(result.years) || result.years.length > 256 || typeof result.partial !== "boolean"
            || result.years.some(year => !Number.isInteger(year) || year < 1000 || year > 9999)) throw "PLEX_DISCOVER_INVALID";
          years = [...new Set(result.years)]; partial = result.partial;
        } catch (cause) { if (!current()) return; error = discoverError(cause); }
        this.publish({ decades: { ...this.state.decades, rows: this.state.decades.rows.map(r => r === row ? { ...r, phase: error ? "error" : "ready", years, partial, error } : r) } });
      });
    }
  }
  openMix(mode: Snapshot["mixMode"] = this.state.mixMode) {
    if (!this.switchMix(mode)) return;
    this.closeDecades(); this.closeStyles();
    if (!this.scope || !this.mixQuery) return;
    if (this.state.mix.phase !== "idle") { this.publish({ mix: { ...this.state.mix, opened: true } }); return; }
    this.generateMix();
  }
  closeMix() { this.closeStyles(); this.closeDecades(); this.cancelMixSave(); this.publish({ mix: { ...this.state.mix, opened: false } }); }
  openMixSave() {
    if (!this.scope || this.mixSaving || this.state.mix.phase !== "ready" || !this.state.mix.tracks.length) return;
    this.mixSaveRevision++; this.mixSaveItems = mixPlaylistItems(this.state.mix.tracks);
    this.publish({ mixSave: { ...emptyMixSave(), opened: true } });
  }
  setMixSaveName(name: string) { if (!this.mixSaving) this.publish({ mixSave: { ...this.state.mixSave, name, error: null } }); }
  cancelMixSave() {
    if (this.mixSaving) return;
    this.mixSaveRevision++; this.mixSaveItems = null; this.publish({ mixSave: emptyMixSave() });
  }
  async saveMix(create: (name: string, items: NewPlaylistItem[]) => Promise<unknown>) {
    if (!this.scope || !this.state.mixSave.opened || !this.mixSaveItems || this.mixSaving) return;
    const name = this.state.mixSave.name.trim();
    if (!name) { this.publish({ mixSave: { ...this.state.mixSave, error: "playlists.nameRequired" } }); return; }
    const items = this.mixSaveItems, scope = this.scope, revision = this.mixSaveRevision;
    const current = () => this.scope === scope && this.mixSaveRevision === revision;
    this.mixSaving = true; this.publish({ mixSave: { ...this.state.mixSave, busy: true, error: null, saved: false } });
    try {
      await create(name, items);
      if (current()) { this.mixSaveItems = null; this.publish({ mixSave: { ...emptyMixSave(), saved: true } }); }
    } catch (cause) {
      if (current()) this.publish({ mixSave: { ...this.state.mixSave, error: cause === "playlists.duplicateName" ? cause : "playlists.createAndAddError" } });
    } finally {
      this.mixSaving = false; this.publish({ mixSave: { ...this.state.mixSave, busy: false } });
    }
  }
  generateMix() {
    if (!this.scope || !this.mixQuery || this.mixSubmitting || this.state.mix.phase === "loading") return;
    const decade = this.state.mixMode === "decade" ? this.state.mixDecade : null;
    if (this.state.mixMode === "decade" && decade === null) return;
    if (this.state.mixMode === "style" && !this.state.mixStyle) return;
    const eligible = this.state.mixMode === "decade" ? this.targets().filter(library => this.state.decades.rows.some(row => row.library.key === library.key && row.years.some(year => Math.floor(year / 10) * 10 === decade))) : this.state.mixMode === "style" ? this.targets().filter(library=>!!this.styleSelection(library.key)) : this.targets();
    const libraries = shuffleMix(eligible, this.random);
    if (!libraries.length) return;
    const limit = Math.max(1, Math.ceil(MIX_LIMIT / libraries.length));
    this.mixRun = { revision: ++this.mixRevision, round: 1, rows: libraries.map(library => ({ library, limit, phase: "idle", tracks: [], more: false, partial: false, error: null })) };
    this.publish({ mix: { ...emptyMix(), opened: true, phase: "loading" } }); this.ensure();
  }
  private ensureMix() {
    const run = this.mixRun; if (!run || !this.mixQuery) return;
    const decade = this.state.mixMode === "decade" ? this.state.mixDecade ?? undefined : undefined;
    const styleRows = this.state.mixMode === "style" ? this.state.styles.rows : [];
    for (const row of run.rows) {
      if (this.queue.length >= 4) break;
      if (row.phase !== "idle") continue;
      row.phase = "loading";
      this.enqueue(`mix:${row.library.key}`, async (scope, current) => {
        const eligible = () => current() && this.mixRun === run && run.revision === this.mixRevision;
        let stage = "ipc", reason = "operation_rejected", observed = 0;
        try {
          const result = await this.mixQuery!(scope.serverId, row.library.key, scope.language, row.limit, decade, this.state.mixMode === "style" ? this.styleSelection(row.library.key) : undefined);
          if (!eligible()) return;
          stage = "frontend"; reason = "response_scope";
          if (result.server_id !== scope.serverId || result.library_id !== row.library.key || result.language !== scope.language) throw "PLEX_DISCOVER_CONTEXT";
          reason = "response_shape";
          observed = Array.isArray(result.tracks) ? Math.min(result.tracks.length, 101) : 0;
          if (!Array.isArray(result.tracks) || result.tracks.length > row.limit || typeof result.partial !== "boolean" || typeof result.has_more !== "boolean") throw "PLEX_DISCOVER_INVALID";
          reason = "track_or_locator_scope";
          if (result.tracks.some(track => track.server_id !== scope.serverId || track.library_id !== row.library.key || !/^\d{1,20}$/.test(track.rating_key)
              || track.media_locator?.kind !== "plex" || track.media_locator.server_id !== scope.serverId || track.media_locator.rating_key !== track.rating_key
              || !/^\/library\/parts\/[^?#\s]+$/.test(track.media_locator.part_key))) throw "PLEX_DISCOVER_CONTEXT";
          const unique = new Map(row.tracks.map(track => [mixTrackKey(track), track]));
          for (const track of result.tracks) unique.set(mixTrackKey(track), { ...track, library_title: row.library.title });
          row.partial ||= result.partial || new Set(result.tracks.map(mixTrackKey)).size < result.tracks.length;
          row.tracks = [...unique.values()].slice(0, row.limit); row.more = result.has_more; row.phase = "ready";
        } catch (error) {
          if (!eligible()) return; row.phase = "error"; row.error = discoverError(error);
          logPlexMixDiagnostic(stage, row.error, reason, observed);
        }
        if (!eligible() || run.rows.some(row => row.phase === "idle" || row.phase === "loading")) return;
        const count = balancedMix(run.rows.map(row => row.tracks)).length;
        const refill = run.rows.filter(row => row.phase === "ready" && row.more && row.tracks.length < MIX_LIMIT);
        if (run.round === 1 && count < MIX_LIMIT && refill.length) {
          run.round = 2;
          const extra = Math.ceil((MIX_LIMIT - count) / refill.length);
          for (const row of refill) { row.limit = Math.min(MIX_LIMIT, row.tracks.length + extra); row.phase = "idle"; }
          return; // One bounded extra random query per eligible library, no scan.
        }
        const tracks = shuffleMix(balancedMix(run.rows.map(row => row.tracks)), this.random);
        this.mixRun = null;
        this.publish({ mix: { ...this.state.mix, phase: !tracks.length && run.rows.some(row => row.error) ? "error" : "ready", tracks,
          partial: tracks.length > 0 && (run.rows.some(row => row.partial || row.error) || decade !== undefined && this.state.decades.rows.some(row => row.partial || row.error) || styleRows.some(row => row.partial || row.error)), limited: tracks.length > 0 && tracks.length < MIX_LIMIT && run.rows.some(row => row.more),
          issues: [...run.rows.filter(row => row.error).map(row => ({ library: row.library.title, error: row.error! })), ...(decade !== undefined ? this.state.decades.rows.filter(row => row.error).map(row => ({ library: row.library.title, error: row.error! })) : []), ...styleRows.filter(row=>row.error).map(row=>({library:row.library.title,error:row.error!}))] } });
      });
    }
  }
  async playMix(play: (tracks: PlexMixTrack[]) => Promise<void>) {
    const mix = this.state.mix; if (!this.scope || mix.phase !== "ready" || !mix.tracks.length || this.mixSubmitting) return;
    const revision = this.mixRevision, scope = this.scope;
    this.mixSubmitting = true; this.publish({ mix: { ...mix, playing: true, playError: false } });
    let failed = false;
    try { await play(mix.tracks); } catch { failed = true; }
    finally {
      this.mixSubmitting = false;
      this.publish({ mix: { ...this.state.mix, playing: false, playError: failed && revision === this.mixRevision && this.scope === scope } });
    }
  }
  private configureRecentWindow() {
    const excluded = this.addedKeys(), signature = JSON.stringify([...excluded].sort());
    if (signature === this.exclusionSignature) return;
    this.exclusionSignature = signature; this.heroRevision++;
    this.queue = this.queue.filter(job => !job.kind.startsWith("recent:"));
    this.recentWindow = Math.min(DISCOVER_HERO_WINDOW, DISCOVER_HERO_LIMIT + excluded.size);
    this.heroRaw = [];
    this.publish({ hero: { ...this.state.hero, phase: "loading", candidates: [] }, libraries: this.state.libraries.map(row => ({ ...row, recentPhase: "idle", recentError: null, recentPartial: false, recentDatesMissing: false })) });
  }
  private chooseHero(candidates: PlexDiscoverItem[], current: PlexDiscoverItem | null) {
    const choices = candidates.filter(item => !current || discoverItemKey(item) !== discoverItemKey(current));
    if (!choices.length) return current;
    const value = this.random(), fraction = Number.isFinite(value) ? Math.max(0, Math.min(value, 1 - Number.EPSILON)) : 0;
    return choices[Math.floor(fraction * choices.length)];
  }
  anotherDiscovery() {
    const hero = this.state.hero;
    if (hero.phase !== "ready") return;
    const item = this.chooseHero(hero.candidates, hero.item);
    if (item !== hero.item) this.publish({ hero: { ...hero, item, selectionCount: hero.candidates.length, selectionPartial: hero.partial } });
  }
  retryHero(libraryId?: string) {
    this.publish({ libraries: this.state.libraries.map(row => (!libraryId || row.library.key === libraryId) && row.recentPhase === "error" ? { ...row, recentPhase: "idle", recentError: null } : row) });
    this.ensure();
  }
  private settleHero() {
    const rows = this.state.libraries, before = this.state.hero;
    const settled = this.shelvesSettled() && rows.every(row => row.recentPhase === "ready" || row.recentPhase === "error");
    const phase = !settled ? "loading" : rows.length > 0 && rows.every(row => row.recentPhase === "error") ? "error" : "ready";
    const partial = rows.some(row => row.phase === "error" || row.recentPhase === "error" || row.recentPartial || (this.scope?.libraryId === "all" && row.recentDatesMissing));
    const item = settled && phase !== "error" && !before.candidates.length ? null : settled && !before.item ? this.chooseHero(before.candidates, null) : before.item;
    this.publish({ hero: { ...before, phase, item, selectionCount: item ? (!before.item ? before.candidates.length : before.selectionCount) : 0,
      selectionPartial: item ? (!before.item ? partial : before.selectionPartial) : false, partial,
      datesMissing: this.scope?.libraryId === "all" && rows.some(row => row.recentDatesMissing) } });
  }
  private loadRecent(row: DiscoverLibraryState) {
    const id = row.library.key, revision = this.heroRevision, windowSize = this.recentWindow;
    this.update(id, { recentPhase: "loading", recentError: null }); this.settleHero();
    this.enqueue(`recent:${id}`, async (scope, current) => {
      const eligible = () => current() && revision === this.heroRevision && this.shelvesSettled();
      try {
        const result = await this.recent(scope.serverId, id, scope.language, windowSize); if (!eligible()) return;
        if (result.server_id !== scope.serverId || result.library_id !== id || result.language !== scope.language) throw "PLEX_DISCOVER_CONTEXT";
        if (!Array.isArray(result.items) || result.items.length > windowSize || result.items.some(item => item.kind !== "album" || !item.album || item.server_id !== scope.serverId || item.library_id !== id)) throw "PLEX_DISCOVER_INVALID";
        const incoming = result.items.map(item => ({ ...item, library_title: row.library.title }));
        this.heroRaw = mergeHeroCandidates(this.heroRaw, incoming, scope.libraryId === "all", windowSize);
        const excluded = this.addedKeys();
        this.publish({ hero: { ...this.state.hero, candidates: this.heroRaw.filter(item => !excluded.has(discoverAlbumKey(item))).slice(0, DISCOVER_HERO_LIMIT) } });
        this.update(id, { recentPhase: "ready", recentError: null, recentPartial: result.partial, recentDatesMissing: result.dates_missing || result.items.some(item => recentDate(item, "added") === null) });
      } catch (error) { if (!eligible()) return; this.update(id, { recentPhase: "error", recentError: discoverError(error) }); }
      this.settleHero();
    });
  }
  retry(libraryId?: string) {
    this.details.clear();
    if (this.state.libraries.some(row => row.recentPhase === "loading")) {
      this.heroRevision++; this.queue = this.queue.filter(job => !job.kind.startsWith("recent:"));
      this.publish({ libraries: this.state.libraries.map(row => row.recentPhase === "loading" ? { ...row, recentPhase: "idle" } : row) });
    }
    this.publish({ libraries: this.state.libraries.map(row => (!libraryId || row.library.key === libraryId) && row.phase !== "loading" ? { ...row, phase: "idle", error: null } : row) });
    this.ensure();
  }
  private loadLibrary(row: DiscoverLibraryState) {
    const id = row.library.key; this.update(id, { phase: "loading", error: null });
    this.enqueue(`page:${id}`, async (scope, current) => {
      try {
        const page = await this.load(scope.serverId, id, scope.language); if (!current()) return;
        if (page.server_id !== scope.serverId || page.library_id !== id || page.language !== scope.language) throw "PLEX_DISCOVER_CONTEXT";
        const hubs = page.hubs.slice(0, 8).map((hub, source_order) => ({ ...this.scopedHub(hub, row.library, 12), source_order }));
        const bounded = { ...page, hubs, partial: page.partial || hubs.length < page.hubs.length || hubs.some(h => h.partial) };
        const visible = artistSuggestionHubs(hubs, scope);
        if (scope.libraryId === "all") this.mergeLibrary(id, visible);
        else this.publish({ hubs: visible });
        // In All, cards live only in globally capped shelves, never N catalogs.
        this.update(id, { phase: "ready", foundHubs: hubs.length, page: scope.libraryId === "all" ? { ...bounded, hubs: [] } : bounded });
      } catch (error) { if (current()) this.update(id, { phase: "error", error: discoverError(error) }); }
    });
  }
  private mergeLibrary(id: string, incoming: PlexDiscoverHub[]) {
    const scope = this.scope!, before = this.state.hubs;
    const recent: PlexDiscoverHub[] = [];
    for (const family of ["added", "played", "played_artists"] as const) {
      const prior = before.find(h => h.recent_family === family);
      const sources = incoming.filter(h => h.recent_family === family);
      if (!prior && !sources.length) continue;
      const items = mergeRecentItems(prior?.items.flatMap(item => item.artist_members ?? [item]).filter(item => item.library_id !== id) ?? [], sources.flatMap(h => h.items), family, scope.libraries, prior?.items.includes(this.pinned!) ? this.pinned : null);
      recent.push({ id: JSON.stringify([scope.serverId, "all", family]), title: "", recent_family: family, items,
        dates_missing: items.some(item => recentDate(item, family) === null), partial: !!prior?.partial || sources.some(h => h.partial) || (prior?.items.length ?? 0) + sources.flatMap(h => h.items).length > DISCOVER_RECENT_LIMIT });
    }
    const order = new Map(scope.libraries.map((library, index) => [library.key, index]));
    const other = [...before.filter(h => !h.recent_family && h.library_id !== id), ...incoming.filter(h => !h.recent_family)];
    // Non-equivalent shelves retain source identities and server row order.
    other.sort((a, b) => (a.source_order ?? 0) - (b.source_order ?? 0)
      || (order.get(a.library_id!) ?? 0) - (order.get(b.library_id!) ?? 0)
      || a.id.localeCompare(b.id));
    const visible = other.slice(0, DISCOVER_OTHER_SHELVES);
    const opened = before.find(h => !h.recent_family && h.items.some(item => item === this.pinned || item === this.state.detail?.item));
    if (opened && !visible.some(h => h.id === opened.id)) {
      if (visible.length === DISCOVER_OTHER_SHELVES) visible.pop();
      visible.push(opened);
    }
    this.publish({ hubs: [...recent, ...visible] });
  }
  private scopedHub(hub: PlexDiscoverHub, library: PlexLibrary, cap: number) {
    const scope = this.scope!;
    if (hub.items.some(item => item.server_id !== scope.serverId || item.library_id !== library.key)) throw "PLEX_DISCOVER_CONTEXT";
    const candidates = (hub.artist_candidates ?? []).slice(0, 12).filter(item => item.kind === "artist" && !item.album && /^\d{1,20}$/.test(item.rating_key) && item.server_id === scope.serverId && item.library_id === library.key
      && hub.items.some(album => album.kind === "album" && album.album?.artist_rating_key === item.rating_key))
      .map(item => ({ ...item, library_title: library.title }));
    const artist_recommendation = hub.artist_recommendation === true && candidates.length === 1 && (hub.artist_candidates?.length ?? 0) === 1
      && hub.items.length > 0 && hub.items.every(item => item.kind === "album" && item.album?.artist_rating_key === candidates[0].rating_key);
    return { ...hub, source_id: hub.id, id: JSON.stringify([scope.serverId, library.key, hub.id]), library_id: library.key, library_title: library.title,
      artist_recommendation, artist_candidates: candidates,
      items: hub.items.slice(0, cap).map(item => ({ ...item, library_title: library.title })), partial: hub.partial || hub.items.length > cap };
  }
  private ownsMixTrack(track: PlexMixTrack) {
    return !!this.scope && this.state.mix.opened && this.state.mix.phase === "ready"
      && this.state.mix.tracks.includes(track) && track.server_id === this.scope.serverId
      && (this.scope.libraryId === "all" || this.scope.libraryId === track.library_id)
      && this.scope.libraries.some(library => library.key === track.library_id);
  }
  mixAlbum(track: PlexMixTrack): PlexAlbum | null {
    if (!this.ownsMixTrack(track) || typeof track.album_rating_key !== "string"
      || !/^\d{1,20}$/.test(track.album_rating_key) || !track.album_title?.trim()) return null;
    return { rating_key: track.album_rating_key, title: track.album_title, artist: track.artist ?? "",
      artist_rating_key: typeof track.artist_rating_key === "string" && /^\d{1,20}$/.test(track.artist_rating_key) ? track.artist_rating_key : undefined,
      server_id: track.server_id, library_id: track.library_id, library_title: track.library_title, thumb: track.thumb ?? undefined };
  }
  mixArtist(track: PlexMixTrack): SelectedArtist | null {
    if (!this.ownsMixTrack(track) || typeof track.artist_rating_key !== "string"
      || !/^\d{1,20}$/.test(track.artist_rating_key) || !track.artist?.trim()) return null;
    return { rating_key: track.artist_rating_key, name: track.artist,
      server_id: track.server_id, library_id: track.library_id, thumb: null };
  }
  owns(item: PlexDiscoverItem) {
    return !!this.scope && (this.state.hero.item === item || this.state.hero.candidates.includes(item) || [...this.state.hubs, ...this.state.libraries.flatMap(row => row.page?.hubs ?? []), ...(this.state.detail?.hub ? [this.state.detail.hub] : [])].some(hub => hub.items.includes(item)));
  }
  album(item: PlexDiscoverItem): PlexAlbum | null {
    if (!this.owns(item) || !item.album) return null;
    this.pinned = item;
    return { ...item.album, server_id: item.server_id, library_id: item.library_id, library_title: item.library_title };
  }
  open(item: PlexDiscoverItem) {
    if (!this.owns(item) || item.kind !== "collection") return;
    const cached = this.details.get(discoverItemKey(item)); this.detailPosition = 0;
    this.publish({ detail: { item, phase: cached ? "ready" : "loading", hub: cached ?? null, error: null } }); if (!cached) this.loadDetail(item);
  }
  artist(item: PlexDiscoverItem): SelectedArtist | null {
    if (!this.owns(item) || item.kind !== "artist") return null;
    this.pinned = item;
    return { rating_key: item.rating_key, name: item.title, server_id: item.server_id, library_id: item.library_id, thumb: item.thumb };
  }
  cancelArtist() { this.queue = this.queue.filter(request => { if (request.kind === "artist") { request.cancel?.(); return false; } return true; }); }
  loadArtist(artist: SelectedArtist): Promise<{ albums: PlexAlbum[]; topTracks: PlexDiscoverArtistContent["top_tracks"] }> {
    if (!this.scope || !this.fullArtist || !/^\d{1,20}$/.test(artist.rating_key) || artist.server_id !== this.scope.serverId
      || (this.scope.libraryId !== "all" && this.scope.libraryId !== artist.library_id) || !this.scope.libraries.some(l=>l.key===artist.library_id)) return Promise.reject("PLEX_DISCOVER_CONTEXT");
    return new Promise((resolve,reject) => this.enqueue("artist", async(scope,current)=>{
      try {
        const result = await this.fullArtist!(scope.serverId,artist.library_id!,artist.rating_key,scope.language);
        if (!current() || result.server_id !== scope.serverId || result.library_id !== artist.library_id || result.artist_id !== artist.rating_key
            || result.albums.some(album=>album.artist_rating_key!==artist.rating_key || (album.server_id && album.server_id!==scope.serverId) || (album.library_id && album.library_id!==artist.library_id))
            || result.top_tracks.some(track=>track.media_locator.kind!=="plex" || track.media_locator.server_id!==scope.serverId)) throw "PLEX_DISCOVER_CONTEXT";
        const library = scope.libraries.find(l=>l.key===artist.library_id)!;
        resolve({ albums: result.albums.map(album=>({...album,server_id:scope.serverId,library_id:library.key,library_title:library.title})), topTracks:result.top_tracks });
      } catch(error) { reject(error); }
    },()=>reject("PLEX_DISCOVER_CONTEXT")));
  }
  retryDetail() { const item = this.state.detail?.item; if (item && this.state.detail?.phase !== "loading") { this.publish({ detail: { item, phase: "loading", hub: null, error: null } }); this.loadDetail(item); } }
  private loadDetail(item: PlexDiscoverItem) {
    this.enqueue("detail", async (scope, current) => {
      try {
        const result = await this.children(scope.serverId, item.library_id, item.kind, item.rating_key, scope.language);
        if (!current() || this.state.detail?.item !== item) return;
        const library = this.state.libraries.find(row => row.library.key === item.library_id)?.library; if (!library) throw "PLEX_DISCOVER_CONTEXT";
        const hub = this.scopedHub(result, library, 24), key = discoverItemKey(item);
        this.details.delete(key); this.details.set(key, hub); if (this.details.size > 4) this.details.delete(this.details.keys().next().value!);
        this.publish({ detail: { item, phase: "ready", hub, error: null } });
      } catch (error) { if (current() && this.state.detail?.item === item) this.publish({ detail: { item, phase: "error", hub: null, error: discoverError(error) } }); }
    });
  }
  back() { this.queue = this.queue.filter(request => request.kind !== "detail"); this.publish({ detail: null }); }
}
