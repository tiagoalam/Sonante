import { classifySmartLocalAlbumFamily, isLocalGenreFamily } from './localGenreFamilies';
import type { TrackMetadata } from '../types/audio';
import type { SmartAlbum, SmartPreview, SmartRules, SmartSelection } from '../types/smartPlaylist';
// Presentation data only. Actual playback retains native source/intent checks.
// No metadata-title deduplication: different recordings may share those tags.
export async function assembleSmartPreview(selection: SmartSelection, rules: SmartRules, read: (album: SmartAlbum) => Promise<TrackMetadata[]>, current: () => boolean = () => true, withinDeadline: () => boolean = () => true, random: () => number = Math.random): Promise<SmartPreview> {
    const shuffled = <T,>(values: readonly T[]): T[] => {
        const result = [...values];
        for (let i = result.length - 1; i > 0; i--) {
            const j = Math.floor(random() * (i + 1));
            [result[i], result[j]] = [result[j], result[i]];
        }
        return result;
    };
    const tracks: TrackMetadata[] = [];
    const seen = new Set<string>();
    const check = () => { if (!current())
        throw 'SMART_PLAYLIST_CANCELLED'; if (!withinDeadline())
        throw 'SMART_PLAYLIST_READ'; };
    check();
    if (rules.genre_family != null && (rules.source.kind !== 'local' || rules.genre || !isLocalGenreFamily(rules.genre_family)))
        throw 'SMART_PLAYLIST_INVALID';
    // Native selection applies the other album rules and sorting. Apply the
    // Local presentation family here using the exact classifier shown in the
    // editor, before any track read or limit. Raw metadata stays untouched.
    const albums = rules.genre_family
        ? selection.albums.filter(album => classifySmartLocalAlbumFamily(album) === rules.genre_family)
        : selection.albums;
    const varied = rules.track_mode === 'varied';
    const albumOrder = new Map(albums.map((album, index) => [album, index] as const));
    const batches: { order: number; tracks: TrackMetadata[] }[] = [];
    for (const album of varied ? shuffled(albums) : albums) {
        check();
        if (varied ? batches.length >= rules.limit : tracks.length >= rules.limit)
            break;
        const batch = await read(album);
        check();
        const unique: TrackMetadata[] = [];
        for (const track of varied ? shuffled(batch) : batch) {
            if (varied ? unique.length >= rules.limit : tracks.length >= rules.limit)
                break;
            const locator = track.media_locator;
            let identity: string;
            if (rules.source.kind === 'local' && locator?.kind === 'local')
                identity = JSON.stringify(['local', locator.uri]);
            else if (rules.source.kind === 'plex' && locator?.kind === 'plex' && locator.server_id === rules.source.server_id)
                identity = JSON.stringify(['plex', locator.server_id, locator.part_key]);
            else if (rules.source.kind === 'navidrome' && locator?.kind === 'navidrome' && locator.account_id === rules.source.account_id)
                identity = JSON.stringify(['navidrome', locator.account_id, locator.song_id]);
            else
                throw 'SMART_PLAYLIST_SOURCE';
            if (!seen.has(identity)) {
                seen.add(identity);
                (varied ? unique : tracks).push(track);
            }
        }
        if (varied && unique.length) batches.push({ order: albumOrder.get(album)!, tracks: unique });
    }
    // Randomize the chosen subset, then present it in the configured album order.
    batches.sort((a, b) => a.order - b.order);
    if (varied) for (let index = 0; tracks.length < rules.limit; index++) {
        check();
        let added = false;
        for (const batch of batches) {
            if (tracks.length >= rules.limit) break;
            if (batch.tracks[index]) { tracks.push(batch.tracks[index]); added = true; }
        }
        if (!added) break;
    }
    check();
    return { tracks, matched_albums: rules.genre_family ? albums.length : selection.matched_albums, limit_reached: tracks.length >= rules.limit };
}
