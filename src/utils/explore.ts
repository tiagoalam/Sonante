import type { LocalAlbum } from "../types/local";
import type { PlexAlbum } from "../types/plex";
import { deriveLocalArtists, localArtistIdentity } from "./localArtistNavigation.ts";
import { derivePlexArtists, plexArtistIdentity } from "./plexArtistNavigation.ts";
import { deriveAlbumGenres, filterAlbumsByGenre } from "./albumGenres.ts";
import { exploreDiagnosticAlias, exploreDiagnosticText, logExploreDiagnostic } from "./frontendDiagnostics.ts";
import { groupExplorePresentation, explorePresentationArtistKey as artistKey } from "./explorePresentation.ts";

export const EXPLORE_PAGE_SIZE = 6;
export const EXPLORE_SUGGESTION_COUNT = 10;
export type ExploreArtistGroup = { kind: "artist"; key: string; name: string; source: "local" | "plex"; artistId: string; ratingKey?: string; albumCount?: number; libraryId?: string; libraryTitle?: string };
export type ExploreGenreGroup = { kind: "genre"; key: string; name: string; value: string; sources: readonly ("local" | "plex")[] };
export type ExploreGroup = ExploreArtistGroup | ExploreGenreGroup;
interface ExploreSlice { group: ExploreGroup; entries: readonly ExploreEntry[]; page: number; }
interface ExploreSnapshot {
  preparing: boolean; selected: ExploreEntry[]; artists: ExploreArtistGroup[]; genres: ExploreGenreGroup[];
  slice: ExploreSlice | null; sliceInvalidated: boolean; revision: number;
}

// Reuse the browser identities and genre validation; names never link Local to Plex.
export function deriveExploreGroups(context: ExploreContext) {
  const artists: ExploreArtistGroup[] = [
    ...deriveLocalArtists(context.local.albums).map(artist => ({ kind: "artist" as const, source: "local" as const,
      key: JSON.stringify(["artist", "local", artist.id]), artistId: artist.id, name: artist.name, albumCount: artist.albumCount })),
    ...plexCatalogs(context).flatMap(catalog => derivePlexArtists(catalog.albums).map(artist => ({ kind: "artist" as const, source: "plex" as const,
      key: JSON.stringify(["artist", "plex", context.plex.serverId, catalog.libraryId, artist.id]), artistId: artist.id, name: artist.name,
      ratingKey: artist.ratingKey, albumCount: artist.albumCount, libraryId: catalog.libraryId, libraryTitle: catalog.albums[0]?.library_title }))),
  ];
  const localGenres = deriveAlbumGenres(context.local.albums), plexGenres = deriveAlbumGenres(context.plex.albums);
  const localValues = new Set(localGenres.map(genre => genre.value)), plexValues = new Set(plexGenres.map(genre => genre.value));
  const genres: ExploreGenreGroup[] = deriveAlbumGenres([...context.local.albums, ...context.plex.albums]).map(genre => ({
    kind: "genre", key: JSON.stringify(["genre", genre.value]), name: genre.label, value: genre.value,
    sources: [...(localValues.has(genre.value) ? ["local" as const] : []), ...(plexValues.has(genre.value) ? ["plex" as const] : [])],
  }));
  return { artists, genres };
}

function sampleGroups<T extends ExploreGroup>(pool: readonly T[], retained: readonly T[], random: () => number): T[] {
  const selected = [...retained].slice(0, 4), candidates = pool.filter(group => !selected.some(old => old.key === group.key));
  for (let i = candidates.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [candidates[i], candidates[j]] = [candidates[j], candidates[i]]; }
  while (selected.length < 4 && candidates.length) {
    const index = candidates.findIndex(group => group.kind === "artist" && !selected.some(old => old.kind === "artist" && old.source === group.source));
    selected.push(candidates.splice(index < 0 ? 0 : index, 1)[0]);
  }
  return selected;
}

export type ExploreSourceState = "ready" | "loading" | "empty" | "not_configured" | "unavailable";
export type ExploreEntry = { key: string; source: "local"; album: LocalAlbum; version: readonly LocalAlbum[] }
  | { key: string; source: "plex"; album: PlexAlbum; version: readonly PlexAlbum[]; identity: unknown; serverId: string; libraryId: string };
export interface ExploreContext {
  local: { albums: readonly LocalAlbum[]; state: ExploreSourceState };
  plex: { albums: readonly PlexAlbum[]; state: ExploreSourceState; identity: unknown; serverId: string; libraryId: string;
    catalogs?: readonly { libraryId: string; albums: readonly PlexAlbum[] }[]; loading?: boolean; completeness?: "complete" | "partial" | "unknown" };
}
const plexCatalogs = (context: ExploreContext) => context.plex.catalogs ?? [{ libraryId: context.plex.libraryId, albums: context.plex.albums }];
function entryContextCurrent(context: ExploreContext, entry: ExploreEntry): boolean {
  if (context[entry.source].state !== "ready") return false;
  return entry.source === "local" ? entry.version === context.local.albums
    : entry.identity === context.plex.identity && entry.serverId === context.plex.serverId
      && plexCatalogs(context).some(catalog => catalog.libraryId === entry.libraryId && catalog.albums === entry.version);
}

export function exploreOriginName(entry: ExploreEntry): string | null {
  const name = entry.source === "local" ? entry.album.source_label : entry.album.library_title;
  return typeof name === "string" && name.trim() ? name.trim() : null;
}

export function exploreEntryIsCurrent(context: ExploreContext, entry: ExploreEntry): boolean {
  return entryContextCurrent(context, entry) && entry.version.some(album => album === entry.album);
}
const entries = (context: ExploreContext): ExploreEntry[] => [
  ...context.local.albums.map(album => ({ key: JSON.stringify(["local", album.id]), source: "local" as const, album, version: context.local.albums })),
  ...plexCatalogs(context).flatMap(catalog => catalog.albums.map(album => ({ key: JSON.stringify(["plex", context.plex.serverId, catalog.libraryId, album.rating_key]), source: "plex" as const,
    album, version: catalog.albums, identity: context.plex.identity, serverId: context.plex.serverId, libraryId: catalog.libraryId }))),
];
const sameContext = (a: ExploreContext, b: ExploreContext) => a.local.albums === b.local.albums && a.local.state === b.local.state
  && a.plex.albums === b.plex.albums && a.plex.state === b.plex.state && a.plex.identity === b.plex.identity
  && a.plex.serverId === b.plex.serverId && a.plex.libraryId === b.plex.libraryId && a.plex.loading === b.plex.loading;

export function pickExploreSuggestions(pool: readonly ExploreEntry[], retained: readonly ExploreEntry[] = [], random = Math.random, preserveRetained = false): ExploreEntry[] {
  const groups = groupExplorePresentation(pool, retained);
  // Once published, a valid card keeps its original source even if a Local copy arrives.
  const candidatesByGroup = groups.map(group => ({ ...group, candidate: preserveRetained
    ? groupExplorePresentation(group.members.filter(entry => retained.some(old => old.key === entry.key)))[0]?.candidate ?? group.candidate
    : group.candidate }));
  const representatives = new Map(candidatesByGroup.flatMap(group => group.members.map(entry => [entry.key, group.candidate] as const)));
  const selected: ExploreEntry[] = [];
  for (const old of retained) {
    const current = representatives.get(old.key);
    if (current && selected.length < EXPLORE_SUGGESTION_COUNT && !selected.includes(current)) selected.push(current);
  }
  // Source variety is chosen when creating/renewing, never by displacing valid cards.
  const candidates = candidatesByGroup.map(group => group.candidate).filter(entry => !selected.includes(entry));
  for (let i = candidates.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [candidates[i], candidates[j]] = [candidates[j], candidates[i]]; }
  while (selected.length < EXPLORE_SUGGESTION_COUNT && candidates.length) {
    const sourceCounts = { local: selected.filter(e => e.source === "local").length, plex: selected.filter(e => e.source === "plex").length };
    // Fill up to ten distinct slots even while another source is still loading.
    const eligible = candidates;
    const absentSource = eligible.find(entry => sourceCounts[entry.source] === 0)?.source;
    const choices = absentSource ? eligible.filter(entry => entry.source === absentSource) : eligible;
    const score = (entry: ExploreEntry) => selected.filter(item => artistKey(item) === artistKey(entry)).length * 10 + sourceCounts[entry.source];
    const best = choices.reduce((a, b) => score(b) < score(a) ? b : a);
    selected.push(best); candidates.splice(candidates.indexOf(best), 1);
  }
  return selected;
}

// Inventory and groups are rebuilt only when catalog arrays or the Plex scope change.
// Visibility/availability invalidate interaction tokens without reshuffling retained identities.
export function createExploreSession(random = Math.random) {
  let context: ExploreContext | null = null, active = true, desiredActive = true;
  let availableInventory: ExploreEntry[] = [];
  let inventory: ExploreEntry[] = [], byKey = new Map<string, ExploreEntry>();
  let groups = { artists: [] as ExploreArtistGroup[], genres: [] as ExploreGenreGroup[] };
  let state: ExploreSnapshot = { preparing: true, selected: [], artists: [], genres: [], slice: null, sliceInvalidated: false, revision: 0 };
  const listeners = new Set<() => void>();
  const publish = (patch: Partial<ExploreSnapshot>, notify = true) => {
    state = { ...state, ...patch, revision: state.revision + 1 }; if (notify) listeners.forEach(fn => fn());
  };
  // byKey already proves membership; avoid rescanning each library for every album.
  const usable = (entry: ExploreEntry) => !!context && byKey.get(entry.key) === entry && entryContextCurrent(context, entry);
  const groupAvailable = (group: ExploreGroup) => !!context && (group.kind === "artist"
    ? context[group.source].state === "ready" && (group.source === "local" || plexCatalogs(context).some(catalog => catalog.libraryId === group.libraryId)) : group.sources.some(source => context![source].state === "ready"));
  const pool = () => availableInventory;
  const matchesGroup = (entry: ExploreEntry, group: ExploreGroup) => group.kind === "genre"
    ? filterAlbumsByGenre([entry.album], group.value).length > 0
    : entry.source === group.source && (entry.source === "local" ? localArtistIdentity(entry.album.artist)
      : entry.libraryId === group.libraryId ? plexArtistIdentity(entry.album) : null) === group.artistId;
  const pageEntries = () => state.slice?.entries.slice(state.slice.page * EXPLORE_PAGE_SIZE, (state.slice.page + 1) * EXPLORE_PAGE_SIZE) ?? [];
  const diagnose = (event: "context" | "visibility" | "renew" | "slice" | "page" | "shelves", inventoryChanged = false) => logExploreDiagnostic(() => {
    const countSources = (items: readonly ExploreEntry[]) => ({ local: items.filter(e => e.source === "local").length, plex: items.filter(e => e.source === "plex").length });
    const visible = state.selected.filter(usable), currentPage = pageEntries();
    const roots = (items: readonly ExploreEntry[]) => {
      const counts: Record<string, number> = {};
      for (const entry of items) if (entry.source === "local") {
        const alias = entry.album.source_id ? exploreDiagnosticAlias("root", entry.album.source_id) : "unknown";
        counts[alias] = (counts[alias] ?? 0) + 1;
      }
      return counts;
    };
    return {
      event, revision: state.revision, inventoryChanged, active: active ? "explore" : "hidden",
      sourceStates: { local: context!.local.state, plex: context!.plex.state },
      plexLibrary: context!.plex.libraryId ? exploreDiagnosticAlias("library", JSON.stringify([context!.plex.serverId, context!.plex.libraryId])) : null,
      plexCompleteness: context!.plex.completeness ?? "unknown",
      albums: { received: countSources(inventory), eligible: countSources(availableInventory), selected: countSources(state.selected), visible: countSources(visible) },
      artists: { derived: groups.artists.length, eligible: groups.artists.filter(groupAvailable).length, sampled: state.artists.length, visible: state.artists.filter(groupAvailable).length },
      genres: { derived: groups.genres.length, eligible: groups.genres.filter(groupAvailable).length, sampled: state.genres.length, visible: state.genres.filter(groupAvailable).length },
      localRoots: { received: roots(inventory), eligible: roots(availableInventory), selected: roots(visible) },
      sample: visible.slice(0, 3).map(e => ({ source: e.source, title: exploreDiagnosticText(e.album.title), artist: exploreDiagnosticText(e.album.artist) })),
      slice: state.slice ? { kind: state.slice.group.kind, eligible: countSources(state.slice.entries), page: state.slice.page + 1,
        pages: Math.ceil(state.slice.entries.length / EXPLORE_PAGE_SIZE), visible: countSources(currentPage),
        sample: currentPage.slice(0, 3).map(e => ({ source: e.source, title: exploreDiagnosticText(e.album.title), artist: exploreDiagnosticText(e.album.artist) })) } : null,
    };
  });
  return {
    getSnapshot: () => state,
    subscribe(fn: () => void) { listeners.add(fn); return () => { listeners.delete(fn); }; },
    syncContext(next: ExploreContext, visible: boolean, notify = true) {
      const changed = !context || !sameContext(context, next), visibilityChanged = active !== visible;
      desiredActive = visible; active = visible;
      if (!changed && !visibilityChanged) { context = next; return; }
      if (!changed) { context = next; publish({}, notify); diagnose("visibility"); return; }
      const oldContext = context;
      const inventoryChanged = !oldContext || oldContext.local.albums !== next.local.albums || oldContext.plex.albums !== next.plex.albums
        || oldContext.plex.identity !== next.plex.identity || oldContext.plex.serverId !== next.plex.serverId || oldContext.plex.libraryId !== next.plex.libraryId;
      const previousInventory = inventory;
      const progressOnly = !!oldContext && !inventoryChanged && oldContext.local.state === next.local.state && oldContext.plex.state === next.plex.state;
      context = next;
      if (inventoryChanged) {
        inventory = entries(next); byKey = new Map(inventory.map(entry => [entry.key, entry]));
        groups = deriveExploreGroups(next);
      }
      availableInventory = inventory.filter(usable);
      const additionOnly = !!oldContext && oldContext.local.albums === next.local.albums && oldContext.local.state === next.local.state
        && oldContext.plex.identity === next.plex.identity && oldContext.plex.serverId === next.plex.serverId
        && (inventory.length > previousInventory.length || plexCatalogs(next).length > plexCatalogs(oldContext).length) && previousInventory.every(old => {
          const current = byKey.get(old.key); return current?.album === old.album && (oldContext[old.source].state !== "ready" || entryContextCurrent(next, current));
        });
      let reconciledSlice = state.slice;
      if (additionOnly && reconciledSlice) {
        const previous = reconciledSlice.entries.map(entry => byKey.get(entry.key)!).filter(usable);
        const previousKeys = new Set(previous.map(entry => entry.key));
        const added = availableInventory.filter(entry => !previousKeys.has(entry.key) && matchesGroup(entry, reconciledSlice!.group));
        reconciledSlice = { ...reconciledSlice, entries: [...previous, ...added] };
      }

      const retained = state.selected.flatMap(old => {
        if (old.source === "plex" && oldContext?.plex.identity !== next.plex.identity) return [];
        const current = byKey.get(old.key);
        return current && usable(current) ? [current] : [];
      });
      const available = pool();
      const artistPool = groups.artists.filter(groupAvailable), genrePool = groups.genres.filter(groupAvailable);
      const retainGroups = <T extends ExploreGroup>(selected: readonly T[], all: readonly T[]) => selected.flatMap(old => {
        if (old.kind === "artist" && old.source === "plex" && oldContext?.plex.identity !== next.plex.identity) return [];
        const current = all.find(group => group.key === old.key); return current && groupAvailable(current) ? [current] : [];
      });
      // Local's first confirmed snapshot settles the initial selection. Plex may continue
      // loading: ready Local must never wait for the aggregate to finish.
      const preparing = state.preparing && (next.local.state === "loading"
        || next.local.state !== "ready" && next.plex.state !== "ready" && (next.plex.state === "loading" || next.plex.loading === true));
      const first = !preparing && (state.preparing || !state.selected.length && available.length > 0);
      const artists = retainGroups(state.artists, groups.artists), genres = retainGroups(state.genres, groups.genres);
      const retainedKeys = new Set(retained.map(entry => entry.key));
      const retainedGroupCount = groupExplorePresentation(available, retained)
        .filter(group => group.members.some(entry => retainedKeys.has(entry.key))).length;
      const invalidated = retainedGroupCount < state.selected.length;
      publish({ preparing,
        selected: preparing ? [] : first || invalidated ? pickExploreSuggestions(available, retained, random, !first) : retained,
        artists: preparing ? [] : first || artists.length < state.artists.length ? sampleGroups(artistPool, artists, random) : artists,
        genres: preparing ? [] : first || genres.length < state.genres.length ? sampleGroups(genrePool, genres, random) : genres,
        ...(changed && state.slice && !progressOnly ? additionOnly ? { slice: reconciledSlice } : { slice: null, sliceInvalidated: true } : {}) }, notify);
      diagnose("context", inventoryChanged);
    },
    renew() {
      if (!active || !context || state.preparing) return;
      const available = groupExplorePresentation(pool(), state.selected).map(group => group.candidate);
      const unselected = available.filter(entry => !state.selected.some(old => old.key === entry.key));
      const seed = unselected.length ? [unselected[Math.floor(random() * unselected.length)]] : [];
      let next = pickExploreSuggestions(available, seed, random);
      if (next.length > 1 && next.map(e => e.key).join() === state.selected.map(e => e.key).join()) next = [...next.slice(1), next[0]];
      const renewGroups = <T extends ExploreGroup>(all: readonly T[], selected: readonly T[]) => {
        const availableGroups = all.filter(groupAvailable), alternatives = availableGroups.filter(group => !selected.some(old => old.key === group.key));
        let renewed = sampleGroups(availableGroups, alternatives.length ? [alternatives[Math.floor(random() * alternatives.length)]] : [], random);
        if (renewed.length > 1 && renewed.every((group, index) => group.key === selected[index]?.key)) renewed = [...renewed.slice(1), renewed[0]];
        return renewed;
      };
      publish({ selected: next, artists: renewGroups(groups.artists, state.artists), genres: renewGroups(groups.genres, state.genres) });
      diagnose("renew");
    },
    canUse(entry: ExploreEntry, revision: number) { return active && revision === state.revision && usable(entry) && state.selected.includes(entry); },
    canUseSlice(entry: ExploreEntry, revision: number) { return active && revision === state.revision && usable(entry) && pageEntries().includes(entry); },
    openSlice(group: ExploreGroup, revision: number) {
      const selectedGroups: readonly ExploreGroup[] = group.kind === "artist" ? state.artists : state.genres;
      if (!active || revision !== state.revision || !selectedGroups.includes(group) || !groupAvailable(group)) return;
      const filtered = pool().filter(entry => matchesGroup(entry, group));
      publish({ slice: { group, entries: filtered, page: 0 }, sliceInvalidated: false });
      diagnose("slice");
    },
    setPage(page: number, revision: number) {
      if (!active || revision !== state.revision || !state.slice || !Number.isInteger(page)
        || page < 0 || page * EXPLORE_PAGE_SIZE >= state.slice.entries.length) return;
      publish({ slice: { ...state.slice, page } });
      diagnose("page");
    },
    leaveSlice(revision: number) {
      if (active && revision === state.revision) { publish({ slice: null, sliceInvalidated: false }); diagnose("shelves"); }
    },
    validDetail(entry: ExploreEntry) { return usable(entry); },
    visibleEntries: () => state.selected.filter(usable),
    visibleArtists: () => state.artists.filter(groupAvailable),
    visibleGenres: () => state.genres.filter(groupAvailable),
    sliceEntries: pageEntries,
    candidateCount: () => pool().length,
    activate() { active = desiredActive; },
    close() { active = false; publish({}, false); },
  };
}
