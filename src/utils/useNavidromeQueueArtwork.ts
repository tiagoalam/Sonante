import { useEffect, useLayoutEffect, useMemo, useState } from 'react';
import type { PlaybackStatus, TrackMetadata } from '../types/audio';
import { navidromeSongCover } from './navidromeArtwork';
import { logError } from './frontendDiagnostics';
import { queueItemArtwork } from './nowPlayingPresentation';
import { navidromeQueueArtworkKey, VisibleNavidromeQueueArtwork } from './visibleNavidromeQueueArtwork';

export function useNavidromeQueueArtwork(items: TrackMetadata[], visibleIndices: readonly number[],
    active: boolean, status?: PlaybackStatus) {
    const queueKey = JSON.stringify(items.map(navidromeQueueArtworkKey));
    const session = useMemo(() => active ? {} : null, [active, queueKey]);
    const [, redraw] = useState(0);
    const controller = useMemo(() => new VisibleNavidromeQueueArtwork(async (connection, song, current) => {
        try { return await navidromeSongCover(connection, song, current); }
        catch (error) { if (current()) logError('artwork.album-load', error); return null; }
    }, () => redraw(value => value + 1)), []);
    useLayoutEffect(() => {
        controller.begin(session);
        return () => controller.begin(null);
    }, [controller, session]);
    const visible = visibleIndices.filter(index => !!items[index])
        .map(index => queueItemArtwork(items[index], index, status));
    // Polling and elapsed changes must not restart completed reads or misses.
    const visibleKey = JSON.stringify(visible.map(item => [navidromeQueueArtworkKey(item), !!item.thumb, !!item.plex_image]));
    useEffect(() => { controller.show(visible); }, [controller, session, visibleKey]);
    return (item: TrackMetadata): TrackMetadata => {
        if (!active || item.thumb || item.plex_image) return item;
        const thumb = controller.get(session, item);
        return thumb ? { ...item, thumb } : item;
    };
}
