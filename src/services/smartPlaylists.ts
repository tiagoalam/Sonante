import { invoke } from '@tauri-apps/api/core';
import { localSourceReadQueue } from '../utils/localSourceReadQueue';
import { assembleSmartPreview } from '../utils/smartPlaylistPreview';
import { audioService } from './audio';
import { plexService } from './plex';
import { navidromeService } from './navidrome';
import { navidromeQueueTrack } from '../utils/navidromeTracks';
import type { SmartPlaylist, SmartRules, SmartSource, SmartAlbum, SmartSources, SmartSelection } from '../types/smartPlaylist';
const alive = () => true;
function check(current: () => boolean) { if (!current())
    throw 'SMART_PLAYLIST_CANCELLED'; }
function sourceRead<T>(source: SmartSource, current: () => boolean, read: () => Promise<T>): Promise<T> {
    const work = () => { check(current); return read(); };
    return source.kind === 'local' ? localSourceReadQueue(work) : Promise.resolve().then(work);
}
export const smartPlaylistsService = {
    list: () => invoke<SmartPlaylist[]>('smart_playlist_store', { action: { kind: 'list' } }),
    save: (id: string | null, name: string, rules: SmartRules) => invoke<SmartPlaylist[]>('smart_playlist_store', { action: { kind: 'save', id, name, rules } }),
    delete: (id: string) => invoke<SmartPlaylist[]>('smart_playlist_store', { action: { kind: 'delete', id } }),
    sources: () => invoke<SmartSources>('smart_playlist_sources'),
    catalog: (source: SmartSource, current: () => boolean = alive) => sourceRead(source, current, () => invoke<SmartAlbum[]>('smart_playlist_catalog', { source })),
    preview: async (rules: SmartRules, current: () => boolean = alive) => {
        const deadline = Date.now() + 120000;
        const nav = rules.source.kind === 'navidrome' ? await navidromeService.status() : null;
        if (rules.source.kind === 'navidrome' && (!nav?.connection_id || nav.account_id !== rules.source.account_id)) throw 'SMART_PLAYLIST_SOURCE';
        const selection = await sourceRead(rules.source, current, () => invoke<SmartSelection>('smart_playlist_preview', { rules }));
        check(current);
        const result = await assembleSmartPreview(selection, rules, async (album) => {
            if (rules.source.kind === 'local') {
                const groups = await sourceRead(rules.source, current, () => audioService.getLocalAlbumTracks(album.id));
                return [...groups].sort((a, b) => a.disc.number - b.disc.number).flatMap(group => group.tracks).filter(t => t.item_type === 'file').map(t => ({ title: t.title ?? t.name, artist: t.artist?.trim() ? t.artist : album.artist, album: t.album ?? album.title, media_locator: { kind: 'local' as const, uri: t.path }, duration: t.duration }));
            }
            if (rules.source.kind === 'navidrome' && nav?.connection_id) {
                check(current);
                const detail = await navidromeService.album(nav.connection_id, album.id);
                check(current);
                return detail.songs.map(song => navidromeQueueTrack(nav, song));
            }
            check(current);
            const values = await plexService.getAlbumTracks(album.id);
            check(current);
            return values.map(t => ({ title: t.title, artist: t.artist?.trim() ? t.artist : album.artist, album: t.album_title ?? album.title, media_locator: t.media_locator, plex_image: t.thumb ?? album.image ?? null, duration: t.duration_ms !== undefined ? t.duration_ms / 1000 : t.duration }));
        }, current, () => Date.now() <= deadline);
        if (rules.source.kind === 'navidrome') {
            check(current);
            const selected = await navidromeService.status();
            check(current);
            if (selected.account_id !== rules.source.account_id || selected.connection_id !== nav?.connection_id) throw 'SMART_PLAYLIST_SOURCE';
        }
        if (rules.source.kind === 'plex') {
            check(current);
            const selected = await invoke<SmartSources>('smart_playlist_sources');
            check(current);
            if (selected.plex_server_id !== rules.source.server_id)
                throw 'SMART_PLAYLIST_SOURCE';
        }
        return result;
    },
};
export function smartError(error: unknown): string {
    const code = typeof error === 'string' ? error : '';
    return code === 'SMART_PLAYLIST_DUPLICATE' ? 'duplicate' : code === 'SMART_PLAYLIST_INVALID' ? 'invalid' : code === 'SMART_PLAYLIST_STORAGE' ? 'storage' : code === 'SMART_PLAYLIST_SOURCE' ? 'source' : 'read';
}
