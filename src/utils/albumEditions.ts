import type { LocalAlbum } from '../types/local';
import type { PlexAlbum } from '../types/plex';
import type { NavidromeAlbum } from '../types/navidrome';

export type EditionSource = 'local' | 'plex' | 'navidrome';
export type EditionEntry =
  | { key: string; source: 'local'; album: LocalAlbum; origin: string }
  | { key: string; source: 'plex'; album: PlexAlbum; origin: string; serverId: string; libraryId: string }
  | { key: string; source: 'navidrome'; album: NavidromeAlbum; origin: string; accountId: string; connectionId: string };
export interface EditionAnchor { title: string; artist: string; source?: EditionSource; album_id?: string }
export interface EditionGroup { key: string; title: string; artist: string; entries: EditionEntry[] }
export interface EditionLane { source: EditionSource; phase: 'loading' | 'ready' | 'unconfigured' | 'error'; entries: EditionEntry[]; partial?: boolean }
const normalize = (value: string) => value.normalize('NFC').trim().replace(/\s+/gu, ' ').toLowerCase();
const generic = new Set(['unknown', 'unknown album', 'unknown artist', 'artista desconhecido', 'desconhecido', 'álbum desconhecido', 'album desconhecido', 'various artists', 'vários artistas', 'varios artistas', 'va', 'v.a.', 'untitled', 'sem título', 'sem titulo', 'n/a']);
const metadataName = (value: string) => normalize(value).replace(/^[\s[({]+|[\s\])}]+$/gu, '').trim();
export function editionMetadataIssue(album: EditionAnchor): 'unknown' | 'suspicious' | null {
  for (const value of [album.artist, album.title]) {
    if (typeof value !== 'string' || !value.trim() || generic.has(metadataName(value))) return 'unknown';
    if (value.length > 1024 || /[\u0000-\u001f\u007f]/u.test(value) || !/[\p{L}\p{N}]/u.test(value)
      || /(?:https?:\/\/|www\.|\b[\p{L}\p{N}_-]+\.(?:blogspot\.com|wordpress\.com|com|net|org|info|biz|com\.br)\b)/iu.test(value)) return 'suspicious';
  }
  return null;
}
const usable = (value: unknown): value is string => typeof value === 'string' && editionMetadataIssue({ title: value, artist: 'Metadata' }) === null;

// Only complete, recognized edition suffixes are removed for suggestions.
// Never strip arbitrary parentheses, live/remix titles, disc numbers, accents,
// punctuation or artist credits. This key is not catalog/file identity.
function editionSuffix(value: string): boolean {
  const text = normalize(value);
  if (!text || /\b(live|ao vivo|concert|remix|demo|tribute|karaoke|re-?recorded|disc\s*\d|cd\s*\d)\b/u.test(text)) return false;
  const descriptors = /\buhqr\b/u.test(text) ? text.replace(/\bap\b/gu, '') : text;
  const remaining = descriptors.replace(/\b(?:16|20|24|32)[-x/](?:44(?:[.,]1)?|48|88(?:[.,]2)?|96|176(?:[.,]4)?|192|352(?:[.,]8)?|384)\b/gu, '').replace(/(?:\b(?:19|20)\d{2}\b|\b\d+(?:st|nd|rd|th)?\s*(?:anniversary|aniversário)\b|\b(?:remaster(?:ed|izado|izada)?|deluxe|expanded|anniversary|edition|edição|edicao|version|versão|versao|special|especial|limited|limitada|legacy|collector'?s?|collection|original|digital|super|bonus|tracks?|audiophile|audiofil[oa]|gold|disc|hybrid|sacd|dsd(?:64|128|256|512)?|mfsl|mofi|dcc|xrcd(?:2|24)?|uhqr|uhqcd|hqcd|shm(?:[ -]?cd|[ -]?sacd)?|blu[ -]?spec(?:[ -]?cd)?|flac|alac|wav|cd|lp|vinyl|mono|stereo|analogue productions|mobile fidelity|sound lab)\b)/gu, '').replace(/[\s.,:;_+&/\-–—()[\]]/gu, '');
  if (remaining === '' && /(?:remaster|deluxe|expanded|anniversary|aniversário|edition|edição|edicao|sacd|dsd|mfsl|mofi|dcc|xrcd|uhqr|uhqcd|hqcd|shm|blu[ -]?spec|flac|alac|wav|\bcd\b|\bmono\b|\bstereo\b|analogue productions|mobile fidelity)/u.test(text)) return true;
  const attribution = text.match(/\s+[-–—]\s+([\p{L}\p{N}_.]{2,40})$/u);
  return !!attribution && /\b(?:uhqr|mfsl|mofi|dcc|xrcd(?:2|24)?|sacd)\b/u.test(text.slice(0, attribution.index))
    && editionSuffix(text.slice(0, attribution.index));
}
export function editionBaseTitle(value: string): string {
  let title = value.trim(), hadEditionSuffix = false;
  for (let i = 0; i < 4; i++) {
    const bracket = title.match(/\s*(?:\(([^()]*)\)|\[([^\[\]]*)\])\s*$/u);
    if (bracket && editionSuffix(bracket[1] ?? bracket[2])) { title = title.slice(0, bracket.index).trim(); hadEditionSuffix = true; continue; }
    const tail = title.match(/\s+[-–—]\s+(.+)$/u);
    if (tail && editionSuffix(tail[1])) { title = title.slice(0, tail.index).trim(); hadEditionSuffix = true; continue; }
    break;
  }
  // A release-year prefix is ignored only when backed by a recognized edition
  // suffix. Bare dated titles and arbitrary parentheses retain their identity.
  if (hadEditionSuffix) title = title.replace(/^(?:19|20)\d{2}\s+[-–—]\s+(?=\S)/u, '');
  return title || value.trim();
}
export function editionWorkKey(album: EditionAnchor): string | null {
  if (!usable(album.artist) || !usable(album.title)) return null;
  const title = editionBaseTitle(album.title);
  return usable(title) ? JSON.stringify([normalize(album.artist), normalize(title)]) : null;
}
export function groupAlbumEditions(entries: readonly EditionEntry[]): EditionGroup[] {
  const groups = new Map<string, EditionGroup>(), seen = new Set<string>();
  for (const entry of entries) {
    if (seen.has(entry.key)) continue; seen.add(entry.key);
    const key = editionWorkKey(entry.album); if (!key) continue;
    let group = groups.get(key);
    if (!group) { group = { key, title: editionBaseTitle(entry.album.title), artist: entry.album.artist, entries: [] }; groups.set(key, group); }
    group.entries.push(entry);
  }
  return [...groups.values()].map(group => ({ ...group, entries: [...group.entries].sort((a, b) => a.source.localeCompare(b.source)
    || a.origin.localeCompare(b.origin) || a.album.title.localeCompare(b.album.title) || a.key.localeCompare(b.key)) }))
    .sort((a, b) => a.artist.localeCompare(b.artist) || a.title.localeCompare(b.title) || a.key.localeCompare(b.key));
}
export function filterEditionGroups(groups: readonly EditionGroup[], query: string, source: EditionSource | 'all'): EditionGroup[] {
  const text = normalize(query);
  return groups.flatMap(group => {
    const entries = source === 'all' ? group.entries : group.entries.filter(entry => entry.source === source);
    if (entries.length < 2 || text && !normalize([group.title, group.artist, ...entries.map(entry => entry.album.title + ' ' + entry.origin)].join(' ')).includes(text)) return [];
    return [{ ...group, entries }];
  });
}

export const ALBUM_EDITIONS_EVENT = 'sonante:album-editions';
export function validEditionAnchor(value: unknown): value is EditionAnchor {
  const album = value as EditionAnchor | null;
  return !!album && typeof album === 'object' && usable(album.title) && usable(album.artist)
    && (album.source === undefined || ['local', 'plex', 'navidrome'].includes(album.source))
    && (album.album_id === undefined || typeof album.album_id === 'string' && album.album_id.length > 0 && album.album_id.length <= 8192 && !/[\u0000-\u001f\u007f]/u.test(album.album_id));
}
const editionAnchor = (anchor: EditionAnchor): EditionAnchor => ({ title: anchor.title, artist: anchor.artist, ...(anchor.source ? { source: anchor.source } : {}), ...(anchor.album_id ? { album_id: anchor.album_id } : {}) });
export function openAlbumEditions(anchor: EditionAnchor) {
  if (validEditionAnchor(anchor)) window.dispatchEvent(new CustomEvent(ALBUM_EDITIONS_EVENT, { detail: editionAnchor(anchor) }));
}
export function subscribeAlbumEditions(navigate: (anchor: EditionAnchor) => void) {
  const listener = (event: Event) => { const anchor: unknown = (event as CustomEvent).detail; if (validEditionAnchor(anchor)) navigate(editionAnchor(anchor)); };
  window.addEventListener(ALBUM_EDITIONS_EVENT, listener);
  return () => window.removeEventListener(ALBUM_EDITIONS_EVENT, listener);
}
