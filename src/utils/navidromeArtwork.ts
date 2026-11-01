import { navidromeService } from '../services/navidrome';
import { navidromeCovers } from './navidromeCovers';

// One bounded image cache shared by the catalog and current-player projection.
// It is scoped to an ephemeral connection and never persisted.
export const navidromeAlbumCover = navidromeCovers(navidromeService.cover);
const pending = new Map<string, Promise<string | null>>();
export async function navidromeSongCover(connection: string, song: string, current: () => boolean): Promise<string | null> {
    if (!current()) return null;
    const key = JSON.stringify([connection, song]);
    let reference = pending.get(key);
    if (!reference) {
        if (pending.size >= 16) throw 'NAVIDROME_UNAVAILABLE';
        reference = navidromeService.songCoverId(connection, song);
        pending.set(key, reference);
        const own = reference;
        void reference.finally(() => { if (pending.get(key) === own) pending.delete(key); }).catch(() => {});
    }
    const id = await reference;
    if (!id || !current()) return null;
    const image = await navidromeAlbumCover(connection, id, current);
    return current() ? image : null;
}
