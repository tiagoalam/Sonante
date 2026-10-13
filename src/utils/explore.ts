import type { LocalAlbum } from "../types/local";
import type { PlexAlbum } from "../types/plex";

export type ExploreSourceState = "ready" | "loading" | "empty" | "not_configured" | "unavailable";
export type ExploreEntry = { key: string; source: "local"; album: LocalAlbum; version: readonly LocalAlbum[] }
  | { key: string; source: "plex"; album: PlexAlbum; version: readonly PlexAlbum[]; identity: unknown };
export interface ExploreContext {
  local: { albums: readonly LocalAlbum[]; state: ExploreSourceState };
  plex: { albums: readonly PlexAlbum[]; state: ExploreSourceState; identity: unknown; serverId: string; libraryId: string };
}
const artistKey = (entry: ExploreEntry) => entry.album.artist.trim().toLocaleLowerCase() || entry.key;
const entries = (context: ExploreContext): ExploreEntry[] => [
  ...context.local.albums.map(album => ({ key: JSON.stringify(["local", album.id]), source: "local" as const, album, version: context.local.albums })),
  ...context.plex.albums.map(album => ({ key: JSON.stringify(["plex", context.plex.serverId, context.plex.libraryId, album.rating_key]), source: "plex" as const,
    album, version: context.plex.albums, identity: context.plex.identity })),
];
const sameContext = (a: ExploreContext, b: ExploreContext) => a.local.albums === b.local.albums && a.local.state === b.local.state
  && a.plex.albums === b.plex.albums && a.plex.state === b.plex.state && a.plex.identity === b.plex.identity
  && a.plex.serverId === b.plex.serverId && a.plex.libraryId === b.plex.libraryId;

export function pickExploreSuggestions(pool: readonly ExploreEntry[], retained: readonly ExploreEntry[] = [], random = Math.random, pendingSource = false): ExploreEntry[] {
  const selected = [...retained].slice(0, 6), candidates = [...pool].filter(entry => !selected.some(item => item.key === entry.key));
  for (let i = candidates.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [candidates[i], candidates[j]] = [candidates[j], candidates[i]]; }
  while (selected.length < 6 && candidates.length) {
    const sourceCounts = { local: selected.filter(e => e.source === "local").length, plex: selected.filter(e => e.source === "plex").length };
    const eligible = candidates.filter(entry => !pendingSource || sourceCounts[entry.source] < 3);
    if (!eligible.length) break;
    const absentSource = eligible.find(entry => sourceCounts[entry.source] === 0)?.source;
    const choices = absentSource ? eligible.filter(entry => entry.source === absentSource) : eligible;
    const score = (entry: ExploreEntry) => selected.filter(item => artistKey(item) === artistKey(entry)).length * 10 + sourceCounts[entry.source];
    const best = choices.reduce((a, b) => score(b) < score(a) ? b : a);
    selected.push(best); candidates.splice(candidates.indexOf(best), 1);
  }
  return selected;
}

// The session samples only on a new visit, a real inventory change/fill, or explicit renewal.
// Availability changes invalidate interactions without reshuffling retained album identities.
export function createExploreSession(random = Math.random) {
  let context: ExploreContext | null = null, active = true, desiredActive = true;
  let state = { selected: [] as ExploreEntry[], revision: 0 };
  const listeners = new Set<() => void>();
  const publish = (selected: ExploreEntry[], notify = true) => {
    state = { selected, revision: state.revision + 1 }; if (notify) listeners.forEach(fn => fn());
  };
  const usable = (entry: ExploreEntry) => !!context && context[entry.source].state === "ready"
    && (entry.source === "local" ? entry.version === context.local.albums && context.local.albums.includes(entry.album)
      : entry.identity === context.plex.identity && entry.version === context.plex.albums && context.plex.albums.includes(entry.album));
  const pool = () => context ? entries(context).filter(usable) : [];
  const pending = () => context?.local.state === "loading" || context?.plex.state === "loading";
  return {
    getSnapshot: () => state,
    subscribe(fn: () => void) { listeners.add(fn); return () => { listeners.delete(fn); }; },
    syncContext(next: ExploreContext, visible: boolean, notify = true) {
      const changed = !context || !sameContext(context, next), visibilityChanged = active !== visible;
      desiredActive = visible; active = visible;
      if (!changed && !visibilityChanged) { context = next; return; }
      const oldContext = context; context = next;
      const currentEntries = entries(next);
      let retained = state.selected.flatMap(old => {
        if (old.source === "plex" && oldContext?.plex.identity !== next.plex.identity) return [];
        const current = currentEntries.find(item => item.key === old.key);
        return current ? [current.version === old.version ? old : current] : [];
      });
      const available = pool();
      // A newly available source can take one slot without reshuffling the other identities.
      if (retained.length === 6 && ["local", "plex"].every(source => available.some(entry => entry.source === source))
        && new Set(retained.map(entry => entry.source)).size === 1) retained = retained.slice(0, 5);
      publish(pickExploreSuggestions(available, retained, random, pending()), notify);
    },
    renew() {
      if (!active || !context) return;
      const available = pool(); if (available.length < 2) return;
      const unselected = available.filter(entry => !state.selected.some(old => old.key === entry.key));
      const seed = unselected.length ? [unselected[Math.floor(random() * unselected.length)]] : [];
      let next = pickExploreSuggestions(available, seed, random, pending());
      if (next.map(e => e.key).join() === state.selected.map(e => e.key).join()) next = [...next.slice(1), next[0]];
      publish(next);
    },
    canUse(entry: ExploreEntry, revision: number) { return active && revision === state.revision && usable(entry) && state.selected.includes(entry); },
    validDetail(entry: ExploreEntry) { return usable(entry); },
    visibleEntries: () => state.selected.filter(usable),
    candidateCount: () => pool().length,
    activate() { active = desiredActive; },
    close() { active = false; publish(state.selected, false); },
  };
}
