import type { LocalAlbum } from "../types/local";
import { deriveLocalArtists, localArtistIdentity } from "./localArtistNavigation.ts";
import { genreComparisonKey } from "./albumGenres.ts";
import { deriveExploreGenres, exploreGenreNames } from "./exploreGenres.ts";
import { exploreDiagnosticAlias, exploreDiagnosticText, logExploreDiagnostic } from "./frontendDiagnostics.ts";
import { groupExplorePresentation, explorePresentationArtistKey as artistKey } from "./explorePresentation.ts";

import { eligibleExploreThemes, themeCandidates, shuffleExploreSections } from "./exploreThemes.ts";
import type { ExploreThemeShelf } from "./exploreThemes.ts";

export type ExploreShelfId = "albums" | "decade" | "genre" | "artists" | "genres";
export const EXPLORE_PAGE_SIZE = 6;
export const EXPLORE_SUGGESTION_COUNT = 10;
export type ExploreArtistGroup = { kind: "artist"; key: string; name: string; source: "local"; artistId: string; albumCount?: number };
export type ExploreGenreGroup = { kind: "genre"; key: string; name: string; value: string; sources: readonly "local"[] };
export type ExploreGroup = ExploreArtistGroup | ExploreGenreGroup;
interface ExploreSlice { group: ExploreGroup; entries: readonly ExploreEntry[]; page: number; }
interface ExploreSnapshot {
  preparing: boolean; selected: ExploreEntry[]; artists: ExploreArtistGroup[]; genres: ExploreGenreGroup[];
  slice: ExploreSlice | null; sliceInvalidated: boolean; revision: number;
  themes: ExploreThemeShelf[]; sectionOrder: ExploreShelfId[]; resets: Partial<Record<ExploreShelfId, number>>;
}

// Reuse Local browser identities; source IDs never come from physical paths.
export function deriveExploreGroups(context: ExploreContext) {
  const artists: ExploreArtistGroup[] = deriveLocalArtists(context.local.albums).map(artist => ({
    kind: "artist", source: "local", key: JSON.stringify(["artist", "local", artist.id]),
    artistId: artist.id, name: artist.name, albumCount: artist.albumCount,
  }));
  const genres: ExploreGenreGroup[] = deriveExploreGenres(context.local.albums).map(genre => ({
    kind: "genre", key: JSON.stringify(["genre", genre.value]), name: genre.label, value: genre.value, sources: ["local"],
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
export type ExploreEntry = { key: string; source: "local"; album: LocalAlbum; version: readonly LocalAlbum[] };
export interface ExploreContext {
  local: { albums: readonly LocalAlbum[]; state: ExploreSourceState; sourceId?: string | null };
}
function entryContextCurrent(context: ExploreContext, entry: ExploreEntry): boolean {
  return context.local.state === "ready" && entry.source === "local" && entry.version === context.local.albums
    && (context.local.sourceId == null || entry.album.source_id === context.local.sourceId);
}
export function exploreOriginName(entry: ExploreEntry): string | null {
  const name = entry.album.source_label;
  return typeof name === "string" && name.trim() ? name.trim() : null;
}
export function exploreEntryIsCurrent(context: ExploreContext, entry: ExploreEntry): boolean {
  return entryContextCurrent(context, entry) && entry.version.some(album => album === entry.album);
}
const entries = (context: ExploreContext): ExploreEntry[] => context.local.albums
  .filter(album => context.local.sourceId == null || album.source_id === context.local.sourceId)
  .map(album => ({ key: JSON.stringify(["local", album.id]), source: "local", album, version: context.local.albums }));
const sameContext = (a: ExploreContext, b: ExploreContext) => a.local.albums === b.local.albums
  && a.local.state === b.local.state && (a.local.sourceId ?? null) === (b.local.sourceId ?? null);

export function pickExploreSuggestions(pool: readonly ExploreEntry[], retained: readonly ExploreEntry[] = [], random = Math.random, preserveRetained = false): ExploreEntry[] {
  const groups = groupExplorePresentation(pool, retained);
  // Once published, a valid card keeps its original source even if another Local copy arrives.
  const candidatesByGroup = groups.map(group => ({ ...group, candidate: preserveRetained
    ? groupExplorePresentation(group.members.filter(entry => retained.some(old => old.key === entry.key)))[0]?.candidate ?? group.candidate
    : group.candidate }));
  const representatives = new Map(candidatesByGroup.flatMap(group => group.members.map(entry => [entry.key, group.candidate] as const)));
  const selected: ExploreEntry[] = [];
  for (const old of retained) {
    const current = representatives.get(old.key);
    if (current && selected.length < EXPLORE_SUGGESTION_COUNT && !selected.includes(current)) selected.push(current);
  }
  // Artist variety is chosen when creating/renewing, never by displacing valid cards.
  const candidates = candidatesByGroup.map(group => group.candidate).filter(entry => !selected.includes(entry));
  for (let i = candidates.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [candidates[i], candidates[j]] = [candidates[j], candidates[i]]; }
  while (selected.length < EXPLORE_SUGGESTION_COUNT && candidates.length) {
    const choices = candidates;
    const score = (entry: ExploreEntry) => selected.filter(item => artistKey(item) === artistKey(entry)).length;
    const best = choices.reduce((a, b) => score(b) < score(a) ? b : a);
    selected.push(best); candidates.splice(candidates.indexOf(best), 1);
  }
  return selected;
}

// Inventory and groups are rebuilt only when catalog arrays or the Local scope change.
// Visibility/availability invalidate interaction tokens without reshuffling retained identities.
export function createExploreSession(random = Math.random, now = () => new Date()) {
  let context: ExploreContext | null = null, active = true, desiredActive = true;
  let availableInventory: ExploreEntry[] = [];
  let genreCounts = new Map<string, number>(), genreDistinctCounts = new Map<string, number>();
  let genreSets = new Map<string, Set<string>>();
  let inventory: ExploreEntry[] = [], byKey = new Map<string, ExploreEntry>();
  let groups = { artists: [] as ExploreArtistGroup[], genres: [] as ExploreGenreGroup[] };
  let state: ExploreSnapshot = { preparing: true, selected: [], artists: [], genres: [], slice: null, sliceInvalidated: false, revision: 0, themes: [], sectionOrder: ["albums", "artists", "genres"], resets: {} };
  const artistAlbums = new Map<string, ExploreEntry>();
  const listeners = new Set<() => void>();
  const publish = (patch: Partial<ExploreSnapshot>, notify = true) => {
    state = { ...state, ...patch, revision: state.revision + 1 };
    for (const key of artistAlbums.keys()) if (!state.artists.some(group => group.key === key)) artistAlbums.delete(key);
    if (notify) listeners.forEach(fn => fn());
  };
  // byKey already proves membership; avoid rescanning each library for every album.
  const usable = (entry: ExploreEntry) => !!context && byKey.get(entry.key) === entry && entryContextCurrent(context, entry);
  const groupAvailable = (group: ExploreGroup) => !!context && (group.kind === "artist"
    ? context.local.state === "ready" : (genreDistinctCounts.get(group.value) ?? 0) >= 5);
  const pool = () => availableInventory;
  const matchesGroup = (entry: ExploreEntry, group: ExploreGroup) => group.kind === "genre"
    ? genreSets.get(entry.key)?.has(group.value) === true
    : localArtistIdentity(entry.album.artist) === group.artistId;
  const renewSelection = (available: readonly ExploreEntry[], previous: readonly ExploreEntry[]) => {
    const oldKeys = new Set(previous.map(entry => entry.key));
    const groups = groupExplorePresentation(available, previous);
    const alternatives = groups.filter(group => !group.members.some(entry => oldKeys.has(entry.key))).map(group => group.candidate);
    const seed = alternatives.length ? [alternatives[Math.floor(random() * alternatives.length)]] : [];
    let selected = pickExploreSuggestions(alternatives.length >= EXPLORE_SUGGESTION_COUNT ? alternatives : available, seed, random);
    if (selected.length > 1 && selected.every((entry, i) => entry.key === previous[i]?.key)) selected = [...selected.slice(1), selected[0]];
    return selected;
  };
  let themesPool: readonly ExploreEntry[] | null = null, themesYear = 0;
  let eligibleThemes = [] as ReturnType<typeof eligibleExploreThemes>;
  const chooseThemes = () => {
    const year = now().getFullYear();
    if (themesPool !== pool() || themesYear !== year) { themesPool = pool(); themesYear = year; eligibleThemes = eligibleExploreThemes(pool(), year); }
    const eligible = eligibleThemes;
    return (["decade", "genre"] as const).flatMap(id => {
      const options = eligible.filter(theme => theme.id === id);
      if (!options.length) return [];
      const theme = options[Math.floor(random() * options.length)];
      const previous = state.themes.find(old => old.id === theme.id && old.value === theme.value);
      const candidates = themeCandidates(pool(), theme, year);
      return [{ ...theme, selected: previous ? renewSelection(candidates, previous.selected) : pickExploreSuggestions(candidates, [], random) }];
    });
  };
  const sectionOrder = (themes: readonly ExploreThemeShelf[]) => shuffleExploreSections<ExploreShelfId>(["albums", "artists", "genres", ...themes.map(theme => theme.id)], random);
  const renewGroups = <T extends ExploreGroup>(all: readonly T[], selected: readonly T[]) => {
    const availableGroups = all.filter(groupAvailable), alternatives = availableGroups.filter(group => !selected.some(old => old.key === group.key));
    let renewed = sampleGroups(availableGroups, alternatives.length ? [alternatives[Math.floor(random() * alternatives.length)]] : [], random);
    if (renewed.length > 1 && renewed.every((group, index) => group.key === selected[index]?.key)) renewed = [...renewed.slice(1), renewed[0]];
    return renewed;
  };
  const pageEntries = () => state.slice?.group.kind === "genre" ? state.slice.entries : state.slice?.entries.slice(state.slice.page * EXPLORE_PAGE_SIZE, (state.slice.page + 1) * EXPLORE_PAGE_SIZE) ?? [];
  const diagnose = (event: "context" | "visibility" | "renew" | "slice" | "page" | "shelves", inventoryChanged = false) => logExploreDiagnostic(() => {
    const countSources = (items: readonly ExploreEntry[]) => ({ local: items.length });
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
      sourceStates: { local: context!.local.state },
      albums: { received: countSources(inventory), eligible: countSources(availableInventory), selected: countSources(state.selected), visible: countSources(visible) },
      artists: { derived: groups.artists.length, eligible: groups.artists.filter(groupAvailable).length, sampled: state.artists.length, visible: state.artists.filter(groupAvailable).length },
      genres: { derived: groups.genres.length, eligible: groups.genres.filter(groupAvailable).length, sampled: state.genres.length, visible: state.genres.filter(groupAvailable).length },
      localRoots: { received: roots(inventory), eligible: roots(availableInventory), selected: roots(visible) },
      sample: visible.slice(0, 3).map(e => ({ source: e.source, title: exploreDiagnosticText(e.album.title), artist: exploreDiagnosticText(e.album.artist) })),
      slice: state.slice ? { kind: state.slice.group.kind, mode: state.slice.group.kind === "genre" ? "continuous" : "paged", eligible: countSources(state.slice.entries), page: state.slice.group.kind === "genre" ? null : state.slice.page + 1,
        pages: state.slice.group.kind === "genre" ? null : Math.ceil(state.slice.entries.length / EXPLORE_PAGE_SIZE), visible: countSources(currentPage),
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
      const previousInventory = inventory;
      const scopeChanged = !!oldContext && (oldContext.local.sourceId ?? null) !== (next.local.sourceId ?? null);
      const inventoryChanged = !oldContext || oldContext.local.albums !== next.local.albums || scopeChanged;
      const progressOnly = !!oldContext && !inventoryChanged && oldContext.local.state === next.local.state;
      if (scopeChanged) {
        state = { ...state, preparing: true, selected: [], artists: [], genres: [], themes: [], slice: null,
          sliceInvalidated: false, resets: Object.fromEntries((["albums", "decade", "genre", "artists", "genres"] as const).map(id => [id, (state.resets[id] ?? 0) + 1])) };
        artistAlbums.clear();
      }
      context = next;
      if (inventoryChanged) {
        inventory = entries(next); byKey = new Map(inventory.map(entry => [entry.key, entry]));
        groups = deriveExploreGroups(next);
        genreSets = new Map(inventory.map(entry => [entry.key, new Set(exploreGenreNames(entry.album).map(genreComparisonKey))]));
      }
      availableInventory = inventory.filter(usable);
      if (inventoryChanged || oldContext?.local.state !== next.local.state) {
        const members = new Map<string, ExploreEntry[]>();
        for (const entry of availableInventory) for (const genre of genreSets.get(entry.key) ?? []) {
          if (!members.has(genre)) members.set(genre, []); members.get(genre)!.push(entry);
        }
        genreCounts = new Map([...members].map(([value, entries]) => [value, entries.length]));
        genreDistinctCounts = new Map([...members].map(([value, entries]) => [value, groupExplorePresentation(entries).length]));
      }
      const additionOnly = !!oldContext && !scopeChanged && oldContext.local.state === next.local.state
        && inventory.length > previousInventory.length && previousInventory.every(old => {
          const current = byKey.get(old.key); return current?.album === old.album && entryContextCurrent(next, current);
        });
      let reconciledSlice = state.slice;
      if (additionOnly && reconciledSlice) {
        const previous = reconciledSlice.entries.map(entry => byKey.get(entry.key)!).filter(usable);
        const previousKeys = new Set(previous.map(entry => entry.key));
        const added = availableInventory.filter(entry => !previousKeys.has(entry.key) && matchesGroup(entry, reconciledSlice!.group));
        reconciledSlice = { ...reconciledSlice, entries: [...previous, ...added] };
      }

      const retained = state.selected.flatMap(old => {
        const current = byKey.get(old.key);
        return current && usable(current) ? [current] : [];
      });
      const available = pool();
      const artistPool = groups.artists.filter(groupAvailable), genrePool = groups.genres.filter(groupAvailable);
      const retainGroups = <T extends ExploreGroup>(selected: readonly T[], all: readonly T[]) => selected.flatMap(old => {
        const current = all.find(group => group.key === old.key); return current && groupAvailable(current) ? [current] : [];
      });
      const preparing = state.preparing && next.local.state === "loading";
      const first = !preparing && (state.preparing || !state.selected.length && available.length > 0);
      const artists = retainGroups(state.artists, groups.artists), genres = retainGroups(state.genres, groups.genres);
      const retainedKeys = new Set(retained.map(entry => entry.key));
      const retainedGroupCount = groupExplorePresentation(available, retained)
        .filter(group => group.members.some(entry => retainedKeys.has(entry.key))).length;
      const invalidated = retainedGroupCount < state.selected.length;
      const themes = preparing ? [] : first ? chooseThemes() : state.themes.flatMap(theme => {
        const candidates = themeCandidates(available, theme, now().getFullYear());
        if (groupExplorePresentation(candidates).length < EXPLORE_SUGGESTION_COUNT) return [];
        const kept = theme.selected.flatMap(old => {
          const current = byKey.get(old.key); return current && usable(current) && candidates.includes(current) ? [current] : [];
        });
        const keys = new Set(kept.map(entry => entry.key));
        const validCount = groupExplorePresentation(candidates, kept).filter(group => group.members.some(entry => keys.has(entry.key))).length;
        return [{ ...theme, selected: validCount < theme.selected.length ? pickExploreSuggestions(candidates, kept, random, true) : kept }];
      });
      publish({ preparing, themes, ...(first ? { sectionOrder: sectionOrder(themes) } : {}),
        selected: preparing ? [] : first || invalidated ? pickExploreSuggestions(available, retained, random, !first) : retained,
        artists: preparing ? [] : first || artists.length < state.artists.length ? sampleGroups(artistPool, artists, random) : artists,
        genres: preparing ? [] : first || genres.length < state.genres.length ? sampleGroups(genrePool, genres, random) : genres,
        ...(changed && state.slice && !progressOnly ? additionOnly ? { slice: reconciledSlice } : { slice: null, sliceInvalidated: true } : {}) }, notify);
      diagnose("context", inventoryChanged);
    },
    renew() {
      if (!active || !context || state.preparing) return;
      const themes = chooseThemes();
      publish({ selected: renewSelection(pool(), state.selected), themes, sectionOrder: sectionOrder(themes),
        resets: Object.fromEntries((["albums", "decade", "genre", "artists", "genres"] as const).map(id => [id, (state.resets[id] ?? 0) + 1])),
        artists: renewGroups(groups.artists, state.artists), genres: renewGroups(groups.genres, state.genres) });
      diagnose("renew");
    },
    renewShelf(id: ExploreShelfId) {
      if (!active || !context || state.preparing) return;
      const reset = { resets: { ...state.resets, [id]: (state.resets[id] ?? 0) + 1 } };
      if (id === "albums") publish({ ...reset, selected: renewSelection(pool(), state.selected) });
      else if (id === "artists") publish({ ...reset, artists: renewGroups(groups.artists, state.artists) });
      else if (id === "genres") publish({ ...reset, genres: renewGroups(groups.genres, state.genres) });
      else {
        const theme = state.themes.find(theme => theme.id === id); if (!theme) return;
        const candidates = themeCandidates(pool(), theme, now().getFullYear());
        if (groupExplorePresentation(candidates).length < EXPLORE_SUGGESTION_COUNT) return;
        publish({ ...reset, themes: state.themes.map(old => old === theme ? { ...theme, selected: renewSelection(candidates, theme.selected) } : old) });
      }
      diagnose("renew");
    },
    canUseShelf(entry: ExploreEntry, revision: number, id: "decade" | "genre") {
      return active && revision === state.revision && usable(entry) && !!state.themes.find(theme => theme.id === id)?.selected.includes(entry);
    },
    genreAlbumCount(group: ExploreGenreGroup) { return genreCounts.get(group.value) ?? 0; },
    artistAlbum(group: ExploreArtistGroup, cached: (entry: ExploreEntry) => boolean = () => false) {
      if (!groupAvailable(group)) return undefined;
      const old = artistAlbums.get(group.key), current = old ? byKey.get(old.key) : undefined;
      if (current && usable(current) && matchesGroup(current, group)) {
        artistAlbums.set(group.key, current); return current;
      }
      // Inspect metadata/cache only; resolve at most this one chosen candidate per card.
      const candidates = pool().filter(entry => matchesGroup(entry, group));
      const chosen = candidates.find(cached) ?? candidates[0];
      if (chosen) artistAlbums.set(group.key, chosen); else artistAlbums.delete(group.key);
      return chosen;
    },
    canUse(entry: ExploreEntry, revision: number) { return active && revision === state.revision && usable(entry) && state.selected.includes(entry); },
    canUseSlice(entry: ExploreEntry, revision: number) { return active && revision === state.revision && usable(entry) && (state.slice?.group.kind === "genre" ? state.slice.entries.includes(entry) : pageEntries().includes(entry)); },
    openSlice(group: ExploreGroup, revision: number) {
      const selectedGroups: readonly ExploreGroup[] = group.kind === "artist" ? state.artists : state.genres;
      if (!active || revision !== state.revision || !selectedGroups.includes(group) || !groupAvailable(group)) return;
      const filtered = pool().filter(entry => matchesGroup(entry, group));
      publish({ slice: { group, entries: filtered, page: 0 }, sliceInvalidated: false });
      diagnose("slice");
    },
    setPage(page: number, revision: number) {
      if (!active || revision !== state.revision || !state.slice || state.slice.group.kind === "genre" || !Number.isInteger(page)
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
