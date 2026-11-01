import type { MediaLocator } from '../types/audio';
import { invoke } from '@tauri-apps/api/core';
import { ensureDirectOutputChoice } from './config';
import { localSourceReadQueue } from '../utils/localSourceReadQueue';
import { playbackIntentToken, type PlaybackIntent } from '../utils/playbackIntent';
import type { NavidromeStatus, NavidromePage, NavidromeDetail, NavidromeSort, NavidromeAlbum, NavidromeSearchResults, NavidromeArtistDetail, NavidromeExplore, NavidromePlayerNavigation } from '../types/navidrome';
export const navidromeService = {
    configuration: () => invoke<boolean>('navidrome_configuration'),
    subscribeConfiguration: async (changed: () => void) => (await import('@tauri-apps/api/event')).listen<void>('navidrome-configuration-changed', changed),
    search: (connectionId:string,query:string) => invoke<NavidromeSearchResults>('navidrome_search',{connectionId,query}),
    artist: (connectionId:string,id:string) => invoke<NavidromeArtistDetail>('navidrome_artist',{connectionId,id}),
    explore: (connectionId:string) => invoke<NavidromeExplore>('navidrome_explore',{connectionId}),
    status: () => invoke<NavidromeStatus>('navidrome_status'),
    connect: (server: string, username: string, password: string) => invoke<NavidromeStatus>('navidrome_connect', { server, username, password }),
    disconnect: () => invoke<NavidromeStatus>('navidrome_disconnect'),
    albums: (connectionId: string, query: string, sort: NavidromeSort, offset: number, pageSize = 36) => invoke<NavidromePage>('navidrome_albums', { connectionId, query, sort, offset, pageSize }),
    favorites: (connectionId: string) => invoke<NavidromeAlbum[]>('navidrome_favorites', { connectionId }),
    setFavorite: (connectionId: string, id: string, enabled: boolean) => invoke<NavidromeAlbum[]>('navidrome_set_favorite', { connectionId, id, enabled }),
    album: (connectionId: string, id: string) => invoke<NavidromeDetail>('navidrome_album', { connectionId, id }),
    resolvePlayerNavigation: (media: {connection_id:string;song_id:string}) => invoke<NavidromePlayerNavigation>('navidrome_player_navigation', {connectionId:media.connection_id,songId:media.song_id}),
    songNavigation: (locator: MediaLocator) => invoke<{account_id:string;album_id:string}>('navidrome_song_navigation', {locator}),
    songCoverId: (connectionId: string, songId: string) => invoke<string | null>('navidrome_song_cover_id', { connectionId, songId }),
    cover: (connectionId: string, id: string) => invoke<string>('navidrome_cover', { connectionId, id }),
    async play(connectionId: string, id: string, startIndex: number, intent: PlaybackIntent) {
        const token = await playbackIntentToken(intent);
        return localSourceReadQueue.control(async () => { if (!intent.current())
            throw 'PLAYBACK_SUPERSEDED'; await ensureDirectOutputChoice(intent); if (!intent.current())
            throw 'PLAYBACK_SUPERSEDED'; return invoke<void>('navidrome_play_album', { connectionId, id, startIndex, intent: token }); });
    },
};
export function navidromeError(error: unknown): string {
    return typeof error === 'string' ? ({ NAVIDROME_AUTH: 'auth', NAVIDROME_CONTEXT_CHANGED: 'changed', NAVIDROME_DETAILS_EXPIRED: 'details', NAVIDROME_INVALID: 'invalid', NAVIDROME_STORAGE: 'storage', NAVIDROME_PROTOCOL: 'protocol', NAVIDROME_RATE_LIMITED: 'limited' } as Record<string, string>)[error] ?? 'unavailable' : 'unavailable';
}
