import { createEditionArchive, createIndexedEditionStorage, editionArchiveFingerprint } from '../utils/albumEditionArchive';
import { createEditionProfileCache, editionDisplayTitle, editionProfile, type EditionProfile } from '../utils/albumEditionAnalysis';
import { createEditionSizeReadQueue } from '../utils/albumEditionSizes';
import { invoke } from '@tauri-apps/api/core';
import { localSourceReadQueue } from '../utils/localSourceReadQueue';
import type { NavidromeAlbum } from '../types/navidrome';
import type { AppConfig } from '../types/config';
import type { EditionEntry, EditionLane, EditionSource } from '../utils/albumEditions';
import { sourceEnabled } from '../utils/musicSources';
import { localAlbumCatalog } from '../utils/localAlbumCatalogSession';
import { plexService } from './plex';
import { navidromeService } from './navidrome';
import { smartPlaylistsService } from './smartPlaylists';
const check = (current: () => boolean) => { if (!current()) throw 'EDITIONS_CANCELLED'; };

// Explicit page work only. Native catalog admission remains authoritative.
// Each lane can finish or fail independently; no track expansion or playback.
export async function loadEditionSource(source: EditionSource, config: AppConfig, current: () => boolean): Promise<EditionLane> {
  check(current);
  if (!sourceEnabled(config, source)) return { source, phase: 'unconfigured', entries: [] };
  if (source === 'local') {
    if (!config.local_folders?.length) return { source, phase: 'unconfigured', entries: [] };
    // Reuse the admitted session catalog used by the browser and artwork. Never
    // start a competing catalog read, or promote saved presentation rows to ready.
    const catalog = localAlbumCatalog.getSnapshot();
    if (!catalog.loaded || catalog.loading || catalog.error || catalog.saveBlocked || catalog.mpdBusy || catalog.manualCheckPlayback) throw 'EDITIONS_NOT_READY';
    const albums = catalog.albums; check(current);
    return { source, phase: 'ready', entries: albums.map(album => ({ key: JSON.stringify(['local', album.id]), source, album, origin: album.source_label ?? '' })) };
  }
  if (source === 'plex') {
    const serverId = config.plex_server_id;
    if (!serverId || !config.plex_token) return { source, phase: 'unconfigured', entries: [] };
    const selected = await smartPlaylistsService.sources(); check(current);
    if (selected.plex_server_id !== serverId) throw 'EDITIONS_SOURCE';
    const libraries = await plexService.getLibraries(); check(current);
    const entries: EditionEntry[] = []; let failed = false;
    for (const library of libraries) {
      check(current);
      try {
        const albums = await plexService.getAlbums(library.key, 'title'); check(current);
        entries.push(...albums.map(album => ({ key: JSON.stringify(['plex', serverId, album.rating_key]), source: 'plex' as const,
          album: { ...album, server_id: serverId, library_id: library.key, library_title: library.title }, origin: library.title, serverId, libraryId: library.key })));
      } catch { check(current); failed = true; }
    }
    const confirmed = await smartPlaylistsService.sources(); check(current);
    if (confirmed.plex_server_id !== serverId) throw 'EDITIONS_SOURCE';
    return { source, phase: failed ? 'error' : 'ready', partial: failed && !!entries.length, entries };
  }
  const nav = await navidromeService.status(); check(current);
  if (!nav.connection_id || !nav.account_id) return { source, phase: 'unconfigured', entries: [] };
  const albums = await invoke<NavidromeAlbum[]>('navidrome_edition_catalog', { connectionId: nav.connection_id, accountId: nav.account_id }); check(current);
  const confirmed = await navidromeService.status(); check(current);
  if (confirmed.connection_id !== nav.connection_id || confirmed.account_id !== nav.account_id) throw 'EDITIONS_SOURCE';
  return { source, phase: 'ready', entries: albums.map(album => ({ key: JSON.stringify(['navidrome', nav.account_id, album.id]), source,
    album, origin: '', accountId: nav.account_id!, connectionId: nav.connection_id! })) };
}

const sizeReads = createEditionSizeReadQueue();
const profileCache = createEditionProfileCache();
const archive = createEditionArchive(createIndexedEditionStorage());
type ArchiveScope = { config: AppConfig; source?: EditionSource; retired: Set<string>; keys: Map<string, string>; fingerprints: Map<string, string>; saved: Map<string, EditionProfile>; available: boolean; renew: boolean; generation: number };
const archiveScopes = new WeakMap<object, ArchiveScope>();
let namespace: Promise<string> | null = null;
const archiveNamespace = () => {
  if (namespace) return namespace;
  const pending = invoke<string>('get_edition_cache_namespace'); namespace = pending;
  void pending.catch(() => { if (namespace === pending) namespace = null; });
  return pending;
};
const archiveContext = (state: ArchiveScope, entry: EditionEntry) => entry.source === 'local' ? state.config.local_folders
  : entry.source === 'plex' ? state.config.plex_token : entry.accountId;
const lastScopes = new Map<string, { identity: object | null; signature: string; scope: object }>();
export function editionReadScope(config: AppConfig, identity: object, navRevision: number, localRevision: number, source?: EditionSource): object {
  const signature = JSON.stringify(source === 'local' ? [sourceEnabled(config, source), config.local_folders, localRevision]
    : source === 'plex' ? [sourceEnabled(config, source), config.plex_server_id, config.plex_token]
    : source === 'navidrome' ? [sourceEnabled(config, source), navRevision]
    : [['local', 'plex', 'navidrome'].map(kind => sourceEnabled(config, kind as EditionSource)), config.local_folders, config.plex_server_id, config.plex_token, navRevision, localRevision]);
  const key = source ?? 'all', owner = !source || source === 'plex' ? identity : null;
  let previous = lastScopes.get(key);
  if (!previous || previous.identity !== owner || previous.signature !== signature) {
    previous = { identity: owner, signature, scope: {} }; lastScopes.set(key, previous);
    archiveScopes.set(previous.scope, { config, source, retired: new Set(), keys: new Map(), fingerprints: new Map(), saved: new Map(), available: true, renew: false, generation: 0 });
  }
  return previous.scope;
}
export function invalidateEditionProfiles(scope: object) {
  profileCache.clear(scope);
  const state = archiveScopes.get(scope);
  if (state) { for (const key of state.keys.values()) state.retired.add(key); state.generation++; state.keys.clear(); state.fingerprints.clear(); state.saved.clear(); state.renew = true; state.available = true; }
}
// Retry incomplete observations only; retain every successful durable result.
export function retryEditionProfiles(entries: readonly EditionEntry[], scope: object) { profileCache.forget(entries, scope); }
export function editionArchiveAvailable(scope: object): boolean { return archiveScopes.get(scope)?.available ?? false; }
export async function restoreEditionProfiles(entries: readonly EditionEntry[], scope: object, current: () => boolean): Promise<Map<string, EditionProfile>> {
  const state = archiveScopes.get(scope), result = new Map<string, EditionProfile>();
  if (!state || !entries.length) return result;
  if (state.source && entries.some(entry => entry.source !== state.source)) throw 'EDITIONS_SOURCE';
  const generation = state.generation, valid = () => current() && generation === state.generation;
  check(valid);
  try {
    const ns = await archiveNamespace(); check(valid);
    const keys = await archive.keys(ns, entries, entry => archiveContext(state, entry)); check(valid);
    if (state.renew) {
      await archive.clear(ns, state.source ? [...new Set([...state.retired, ...keys.values()])] : undefined); check(valid);
      state.retired.clear(); state.renew = false;
    }
    const saved = await archive.restore([...keys.values()]); check(valid);
    for (const [key, hash] of keys) state.keys.set(key, hash);
    for (const entry of entries) state.fingerprints.set(entry.key, editionArchiveFingerprint(entry, archiveContext(state, entry)));
    for (const [hash, profile] of saved) state.saved.set(hash, profile);
    const retained = new Set(state.keys.values());
    for (const hash of state.saved.keys()) if (!retained.has(hash)) state.saved.delete(hash);
    state.available = true;
    for (const [key, hash] of keys) { const profile = saved.get(hash); if (profile) result.set(key, profile); }
  } catch { check(valid); state.available = false; }
  return result;
}
export function cachedEditionProfiles(entries: readonly EditionEntry[], scope: object): Map<string, EditionProfile | null> {
  const result = new Map<string, EditionProfile | null>();
  for (const entry of entries) {
    const profile = profileCache.peek(entry, scope);
    if (profile) result.set(entry.key, profile);
  }
  return result;
}
// One finite album only. The same response
// supplies track-list evidence and bytes; no extra request for each criterion.
export async function readEditionProfile(entry: EditionEntry, scope: object, current: () => boolean): Promise<EditionProfile | null> {
  check(current);
  const owner = archiveScopes.get(scope);
  if (owner?.source && owner.source !== entry.source) throw 'EDITIONS_SOURCE';
  const source = entry.source === 'local' ? { kind: 'local', album_id: entry.album.id }
    : entry.source === 'plex' ? { kind: 'plex', server_id: entry.serverId, library_id: entry.libraryId, album_id: entry.album.rating_key }
    : { kind: 'navidrome', account_id: entry.accountId, connection_id: entry.connectionId, album_id: entry.album.id };
  return profileCache.read(entry, scope, current, async () => {
    const state = archiveScopes.get(scope), generation = state?.generation;
    const valid = () => current() && generation === state?.generation;
    const fingerprint = state ? editionArchiveFingerprint(entry, archiveContext(state, entry)) : null;
    const key = state?.fingerprints.get(entry.key) === fingerprint ? state?.keys.get(entry.key) : null;
    const saved = key ? state?.saved.get(key) : null;
    if (saved) return saved;
    const read = () => { check(valid); return invoke('get_edition_profile', { source }); };
    const raw = await sizeReads(() => entry.source === 'local' ? localSourceReadQueue(read) : read(), valid); check(valid);
    const value = editionProfile(raw);
    // Persist a folder hint only when it is a recognized edition of this exact
    // work. Unrelated directory names never enter the durable observations.
    if (value?.edition_title && editionDisplayTitle(entry, value) !== value.edition_title) delete value.edition_title;
    if (value?.sizes && state?.available && key) {
      // Commit each successful observation BEFORE reporting progress. Closing
      // midway loses at most the in-flight reads, not an entire library pass.
      try { await archive.save(key, value); check(valid); state.saved.set(key, value); }
      catch { check(valid); state.available = false; }
      check(valid);
    }
    return value;
  });
}
