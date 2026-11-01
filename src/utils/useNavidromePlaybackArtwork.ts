import { useLayoutEffect, useState } from 'react';
import type { PlaybackStatus } from '../types/audio';
import { currentMediaKey } from './playerNavigation';
import { navidromeSongCover } from './navidromeArtwork';
import { logError } from './frontendDiagnostics';

// Presentation only: raw telemetry, logical queue and native queue cache remain unchanged.
export function useNavidromePlaybackArtwork(status: PlaybackStatus, available: boolean, active: boolean): PlaybackStatus {
    const media = status.current_media;
    const connection = media?.kind === 'navidrome' ? media.connection_id : null;
    const song = media?.kind === 'navidrome' ? media.song_id : null;
    const key = currentMediaKey(media);
    const [resolved, setResolved] = useState<{ key: string; thumb: string } | null>(null);
    useLayoutEffect(() => {
        if (!connection || !song || !active || !available || status.thumb || status.plex_image) return;
        let live = true;
        void navidromeSongCover(connection, song, () => live).then(thumb => {
            if (live && thumb) setResolved({ key, thumb });
        }).catch(error => { if (live) logError('artwork.album-load', error); });
        return () => { live = false; };
    }, [connection, song, key, active, available, status.thumb, status.plex_image]);
    if (!connection || status.thumb || status.plex_image || resolved?.key !== key) return status;
    return { ...status, thumb: resolved.thumb };
}
