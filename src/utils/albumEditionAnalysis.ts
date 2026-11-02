import { editionSizeVariants, validFileSizes, type EditionVariant } from './albumEditionSizes';
import { editionBaseTitle, type EditionEntry, type EditionGroup } from './albumEditions';

export interface EditionProfile {
  sizes: number[] | null;
  titles: string[] | null;
  cover_id: string | null;
  edition_title?: string;
}
export type EditionProfiles = ReadonlyMap<string, EditionProfile | null>;
export type EditionReason = 'title' | 'tracks';
const normalized = (value: string) => value.normalize('NFC').trim().replace(/\s+/gu, ' ').toLowerCase();
const usableTitle = (v: unknown): v is string => typeof v === 'string' && v.trim().length > 0 && v.length <= 1024 && !/[\u0000-\u001f\u007f]/u.test(v)
  && !/^(?:\[|\()?\s*(?:unknown(?: track| title)?|untitled|sem título|desconhecid[oa])\s*(?:\]|\))?$/iu.test(v.trim());

// A projection of descriptive metadata, never a read or playback capability.
export function editionProfile(value: unknown): EditionProfile | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const raw = value as Record<string, unknown>;
  const titles = Array.isArray(raw.titles) && raw.titles.length > 0 && raw.titles.length <= 500 && raw.titles.every(usableTitle)
    ? raw.titles.map(title => title.trim()) : null;
  const sizes = validFileSizes(raw.sizes) ? [...raw.sizes] : null;
  // A partial inventory must not establish equality or a track-list difference.
  if (sizes && titles && sizes.length !== titles.length) return null;
  const cover_id = typeof raw.cover_id === 'string' && /^[A-Za-z0-9._-]{1,512}$/u.test(raw.cover_id) ? raw.cover_id : null;
  const edition_title = typeof raw.edition_title === 'string' && usableTitle(raw.edition_title)
    && !/[\\/]/u.test(raw.edition_title) ? raw.edition_title.trim() : undefined;
  return sizes || titles ? { sizes, titles, cover_id, ...(edition_title ? { edition_title } : {}) } : null;
}

// Only a recognized edition suffix of this SAME album can enrich normalized
// server metadata. A folder name is descriptive evidence, never a path capability.
export function editionDisplayTitle(entry: EditionEntry, profile?: EditionProfile | null): string {
  const title = profile?.edition_title;
  return title && normalized(editionBaseTitle(title)) === normalized(editionBaseTitle(entry.album.title))
    && normalized(editionBaseTitle(title)) !== normalized(title) ? title : entry.album.title;
}
export function analyzeEditionGroup(group: EditionGroup, profiles: EditionProfiles): { variants: EditionVariant[]; reasons: EditionReason[]; copiesOnly: boolean } {
  const variants = editionSizeVariants(group, new Map(group.entries.map(entry => [entry.key, profiles.get(entry.key)?.sizes ?? null])));
  // Only a complete matching observation of EVERY occurrence can discard a
  // group. One checked batch, missing sizes or a singleton is not sufficient.
  const copiesOnly = group.entries.length > 1 && variants.length === 1 && variants[0].sizes !== null;
  if (variants.length < 2) return { variants, reasons: [], copiesOnly };
  const reasons: EditionReason[] = [];
  // Matching bytes can group copies even when their servers spell tags differently.
  // A different size on its own is deliberately NOT evidence of another edition.
  const labels = new Set(variants.map(variant => normalized(editionDisplayTitle(variant.entries[0], profiles.get(variant.entries[0].key)))));
  if (labels.size > 1 && variants.some(variant => normalized(editionBaseTitle(editionDisplayTitle(variant.entries[0], profiles.get(variant.entries[0].key)))) !== normalized(editionDisplayTitle(variant.entries[0], profiles.get(variant.entries[0].key))))) reasons.push('title');
  const inventories = new Set<string>(), counts = new Set<number>();
  for (const variant of variants) {
    const entry = variant.entries[0], profile = profiles.get(entry.key);
    if (profile?.titles) inventories.add(JSON.stringify(profile.titles.map(normalized).sort()));
    const count = profile?.titles?.length ?? (entry.source === 'local' ? entry.album.track_count : entry.source === 'navidrome' ? entry.album.song_count : null);
    if (typeof count === 'number' && Number.isSafeInteger(count) && count > 0) counts.add(count);
  }
  if (inventories.size > 1 || counts.size > 1) reasons.push('tracks');
  return { variants, reasons, copiesOnly };
}

export const EDITION_CHECK_MEMBERS = 8;
export function editionCandidateKey(group: EditionGroup): string {
  // Source filters can produce different memberships for the same work.
  return JSON.stringify([group.key, group.entries.map(entry => entry.key)]);
}
export function editionComparisonEntries(group: EditionGroup, limit: number): EditionEntry[] {
  const signatures = new Set<string>(), representatives: EditionEntry[] = [], rest: EditionEntry[] = [];
  for (const entry of group.entries) {
    const count = entry.source === 'local' ? entry.album.track_count : entry.source === 'navidrome' ? entry.album.song_count : null;
    const signature = JSON.stringify([normalized(entry.album.title), count ?? null]);
    if (signatures.has(signature)) rest.push(entry);
    else { signatures.add(signature); representatives.push(entry); }
  }
  return [...representatives, ...rest].slice(0, limit);
}
export interface VerifiedEditionGroup {
  group: EditionGroup;
  profiles: Map<string, EditionProfile | null>;
  reasons: EditionReason[];
}
// Track correspondence confirms work identity BEFORE edition differences are
// considered. No broad fuzzy matching: meaningful words, live/remix labels and
// part numbers remain distinct. Each title occurrence is consumed at most once.
export function editionTrackName(value: string): string {
  const yearPrefix = value.match(/^((?:19|20)\d{2}\s+[-–—]\s+)/u)?.[1];
  const base = editionBaseTitle(value);
  return ((yearPrefix && !base.startsWith(yearPrefix) ? yearPrefix : '') + base).normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase()
    .replace(/^\s*\d{1,3}[.)-]\s+/u, '').replace(/[^\p{L}\p{N}]+/gu, ' ').trim().replace(/\s+/gu, ' ');
}
export function sameAlbumTracks(a: readonly string[] | null | undefined, b: readonly string[] | null | undefined): boolean {
  if (!a?.length || !b?.length || a.length > 500 || b.length > 500) return false;
  return correspondingTrackNames(a.map(editionTrackName), b.map(editionTrackName));
}
function correspondingTrackNames(left: readonly string[], right: readonly string[]): boolean {
  if (left.some(title => !title) || right.some(title => !title)) return false;
  const counts = new Map<string, number>();
  for (const title of right) counts.set(title, (counts.get(title) ?? 0) + 1);
  let matched = 0; const distinct = new Set<string>();
  for (const title of left) {
    const available = counts.get(title) ?? 0;
    if (available > 0) { matched++; distinct.add(title); counts.set(title, available - 1); }
  }
  const smaller = Math.min(left.length, right.length);
  if (distinct.size < Math.min(3, smaller)) return false;
  // Do not turn a one-/two-/three-track single or excerpt into an edition of a
  // much larger album merely because it is completely contained in that album.
  if (smaller <= 3) return left.length === right.length && matched === smaller;
  if (smaller === 4) return matched === 4;
  return matched / smaller >= 0.7;
}
export function verifiedEditionGroups(group: EditionGroup, profiles: EditionProfiles): VerifiedEditionGroup[] {
  const known = { ...group, entries: group.entries.filter(entry => validFileSizes(profiles.get(entry.key)?.sizes)) };
  const variants = analyzeEditionGroup(known, profiles).variants.flatMap(variant => {
    const titles = variant.entries.map(entry => profiles.get(entry.key)?.titles).find(value => value?.length);
    return titles ? [{ variant, titles, names: titles.map(editionTrackName) }] : [];
  }).sort((a, b) => a.titles.length - b.titles.length || a.variant.entries[0].key.localeCompare(b.variant.entries[0].key));
  const families: typeof variants[] = [];
  for (const candidate of variants) {
    // Complete linkage prevents a deluxe/compilation bridge from joining two
    // unrelated homonymous albums through transitive partial overlap.
    const fits = families.filter(family => family.every(other => correspondingTrackNames(candidate.names, other.names)));
    if (fits.length === 1) fits[0].push(candidate);
    else if (!fits.length) families.push([candidate]);
    // More than one compatible family is ambiguous; do not guess.
  }
  return families.flatMap(family => {
    if (family.length < 2) return [];
    const entries = family.flatMap(item => item.variant.entries), result = analyzeEditionGroup({ ...group, entries }, profiles);
    if (!result.reasons.length) return [];
    const key = JSON.stringify([group.key, 'track-family', [...entries.map(entry => entry.key)].sort()[0]]);
    return [{ group: { ...group, key, entries }, profiles: new Map(entries.map(entry => [entry.key, profiles.get(entry.key)!])), reasons: result.reasons }];
  });
}
// Compatibility for callers needing one result; the Editions UI consumes all
// families so two real homonymous albums with their own editions stay separate.
export function verifiedEditionGroup(group: EditionGroup, profiles: EditionProfiles): VerifiedEditionGroup | null {
  return verifiedEditionGroups(group, profiles)[0] ?? null;
}
export async function checkEditionCandidates(groups: readonly EditionGroup[], read: (entry: EditionEntry, current: () => boolean) => Promise<EditionProfile | null>,
  current: () => boolean, publish: (group: EditionGroup, matches: VerifiedEditionGroup[], unavailable: boolean, missing: EditionEntry[]) => void): Promise<void> {
  // Sequential albums share the existing two-call read budget. Pause and
  // navigation stop before the next read, without controlling playback.
  for (const group of groups) {
    if (!current()) return;
    const entries = editionComparisonEntries(group, EDITION_CHECK_MEMBERS), profiles = new Map<string, EditionProfile | null>();
    await compareEditionProfiles(entries, read, current, (entry, value) => profiles.set(entry.key, value));
    if (!current()) return;
    const missing = entries.filter(entry => !validFileSizes(profiles.get(entry.key)?.sizes));
    publish(group, verifiedEditionGroups({ ...group, entries }, profiles), missing.length > 0, missing);
  }
}

// Bounded session cache. It survives page remounts, but source/account/catalog
// scope changes and explicit refresh retire results. A short TTL handles remote
// changes for which the server supplies no revision. Nothing touches startup.
export function createEditionProfileCache(now = Date.now) {
  const scopes = new WeakMap<object, Map<string, { at: number; value: EditionProfile; weight: number }>>();
  const keyFor = (entry: EditionEntry) => JSON.stringify([entry.key, entry.album, entry.source === 'navidrome' ? entry.connectionId : null]);
  const peek = (entry: EditionEntry, scope: object): EditionProfile | null => {
    const cache = scopes.get(scope), key = keyFor(entry), cached = cache?.get(key);
    if (!cached) return null;
    const age = now() - cached.at;
    if (age < 0 || age >= (cached.value.sizes ? 300_000 : 30_000)) { cache!.delete(key); return null; }
    return editionProfile(cached.value);
  };
  return {
    clear(scope: object) { scopes.delete(scope); },
    forget(entries: readonly EditionEntry[], scope: object) { const cache = scopes.get(scope); for (const entry of entries) cache?.delete(keyFor(entry)); },
    // Cache-only lookup: classification never causes native work or extends TTL.
    peek,
    async read(entry: EditionEntry, scope: object, current: () => boolean, load: () => Promise<unknown>): Promise<EditionProfile | null> {
      if (!current()) throw 'EDITIONS_CANCELLED';
      let cache = scopes.get(scope);
      if (!cache) { cache = new Map(); scopes.set(scope, cache); }
      const key = keyFor(entry), cached = peek(entry, scope);
      if (cached) {
        const record = cache.get(key)!; cache.delete(key); cache.set(key, record);
        return cached;
      }
      cache.delete(key);
      const value = editionProfile(await load());
      if (!current()) throw 'EDITIONS_CANCELLED';
      if (value) {
        const weight = 2 * (key.length + JSON.stringify(value).length);
        cache.set(key, { at: now(), value, weight });
        let bytes = [...cache.values()].reduce((sum, item) => sum + item.weight, 0);
        while (cache.size > 128 || bytes > 2 * 1024 * 1024) {
          const first = cache.keys().next().value!; bytes -= cache.get(first)!.weight; cache.delete(first);
        }
      }
      return value ? editionProfile(value) : null;
    },
  };
}

export async function compareEditionProfiles(entries: readonly EditionEntry[], read: (entry: EditionEntry, current: () => boolean) => Promise<EditionProfile | null>,
  current: () => boolean, publish: (entry: EditionEntry, profile: EditionProfile | null) => void): Promise<void> {
  const unique = [...new Map(entries.map(entry => [entry.key, entry])).values()]; let next = 0;
  const worker = async () => {
    while (current() && next < unique.length) {
      const entry = unique[next++]; let profile: EditionProfile | null = null;
      try { profile = await read(entry, current); } catch { /* Keep unavailable evidence inconclusive. */ }
      if (!current()) return;
      publish(entry, profile);
    }
  };
  await Promise.all([worker(), worker()]);
}
