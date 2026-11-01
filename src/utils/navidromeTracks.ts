import type { NavidromeStatus, NavidromeSong } from '../types/navidrome';
import type { TrackMetadata } from '../types/audio';
/** Metadata is a display snapshot. Account + song ID are the durable media identity. */
export function navidromeQueueTrack(status: NavidromeStatus, song: NavidromeSong): TrackMetadata {
    if (!status.connection_id || !status.account_id || !/^[a-f0-9]{64}$/.test(status.account_id)) throw 'NAVIDROME_CONTEXT_CHANGED';
    return {title:song.title,artist:song.artist,album:song.album,duration:song.duration ?? undefined,
        media_locator:{kind:'navidrome',connection_id:status.connection_id,account_id:status.account_id,song_id:song.id}};
}
