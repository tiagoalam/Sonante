import { isLocalGenreFamily, smartLocalGenreOptions } from './localGenreFamilies';
import type { SmartRules, SmartSource, SmartAlbum } from '../types/smartPlaylist';
export const defaultSmartRules = (source: SmartSource = { kind: 'local' }): SmartRules => ({ source, genre: null, genre_family: null, track_mode: 'varied', artist: null, year_from: null, year_to: null, added_days: null, favorites_only: false, sort: 'album', limit: 100 });
export function smartRulesValid(r: SmartRules): boolean {
    const year = (n: number | null) => n === null || Number.isInteger(n) && n >= 1000 && n <= 9999;
    return (r.genre_family == null || r.source.kind === 'local' && !r.genre && isLocalGenreFamily(r.genre_family))
        && (r.track_mode === undefined || r.track_mode === 'varied' || r.track_mode === 'album')
        && (r.source.kind === 'local' || r.source.kind === 'navidrome' && /^[a-f0-9]{64}$/.test(r.source.account_id) || r.source.kind === 'plex' && !!r.source.server_id && /^\d{1,32}$/.test(r.source.library_id)) && year(r.year_from) && year(r.year_to)
        && (r.year_from === null || r.year_to === null || r.year_from <= r.year_to) && Number.isInteger(r.limit) && r.limit >= 1 && r.limit <= 500
        && (r.added_days === null || Number.isInteger(r.added_days) && r.added_days >= 1 && r.added_days <= 3650);
}
export function smartTemplate(template: string, rules: SmartRules, albums: readonly SmartAlbum[]): SmartRules {
    const base = defaultSmartRules(rules.source);
    if (template === 'recent')
        return { ...base, added_days: 30, sort: 'added_newest' };
    if (template === 'favorites')
        return { ...base, favorites_only: true, sort: 'artist' };
    if (template === 'genre')
        return rules.source.kind === 'local'
            ? { ...base, genre_family: smartLocalGenreOptions(albums).find(family => family !== 'other') ?? smartLocalGenreOptions(albums)[0] ?? null, sort: 'artist' }
            : { ...base, genre: smartOptions(albums, 'genres')[0] ?? null, sort: 'artist' };
    if (template === 'decade')
        return { ...base, year_from: 1970, year_to: 1979, sort: 'year_oldest' };
    return base;
}
export function smartOptions(albums: readonly SmartAlbum[], field: 'genres' | 'artist'): string[] {
    const values = new Map<string, string>();
    for (const a of albums)
        for (const value of field === 'genres' ? a.genres : [a.artist]) {
            const text = value.trim();
            if (text && !values.has(text.toLowerCase()))
                values.set(text.toLowerCase(), text);
        }
    return [...values.values()].sort((a, b) => a.localeCompare(b));
}
