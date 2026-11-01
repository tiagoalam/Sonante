import type { TrackMetadata } from '../types/audio';

type Source = { connection_id: string; song_id: string };
type Entry = { source: Source; started: boolean; thumb: string | null };
export function navidromeQueueArtworkKey(item: TrackMetadata): string | null {
    const media = item.media_locator;
    return media?.kind === 'navidrome' && media.connection_id && media.song_id
        ? JSON.stringify([media.connection_id, media.song_id]) : null;
}

// Presentation only. Keep just the visible rows/overscan, with two outstanding
// reads. An obsolete IPC still occupies its slot until it settles.
export class VisibleNavidromeQueueArtwork {
    private session: object | null = null;
    private entries = new Map<string, Entry>();
    private running = 0;
    private readonly load: (connection: string, song: string, current: () => boolean) => Promise<string | null>;
    private readonly changed: () => void;
    constructor(load: (connection: string, song: string, current: () => boolean) => Promise<string | null>, changed: () => void) {
        this.load = load;
        this.changed = changed;
    }
    begin(session: object | null) {
        this.session = session;
        this.entries.clear();
    }
    show(items: readonly TrackMetadata[]) {
        if (!this.session) return;
        const visible = new Map<string, Source>();
        for (const item of items) {
            const key = navidromeQueueArtworkKey(item), media = item.media_locator;
            if (key && media?.kind === 'navidrome' && !item.thumb && !item.plex_image)
                visible.set(key, media);
        }
        for (const key of this.entries.keys()) if (!visible.has(key)) this.entries.delete(key);
        for (const [key, source] of visible) if (!this.entries.has(key))
            this.entries.set(key, { source, started: false, thumb: null });
        this.pump();
    }
    get(session: object | null, item: TrackMetadata): string | null {
        const key = navidromeQueueArtworkKey(item);
        return session && session === this.session && key ? this.entries.get(key)?.thumb ?? null : null;
    }
    private pump() {
        if (!this.session) return;
        for (const [key, entry] of this.entries) {
            if (this.running >= 2) break;
            if (entry.started) continue;
            const session = this.session;
            const current = () => session === this.session && this.entries.get(key) === entry;
            entry.started = true;
            this.running++;
            void Promise.resolve().then(() => current()
                ? this.load(entry.source.connection_id, entry.source.song_id, current) : null)
                .then(thumb => { if (current() && thumb) { entry.thumb = thumb; this.changed(); } })
                // Artwork is optional; the caller logs only live, sanitized failures.
                .catch(() => {})
                .finally(() => { this.running--; this.pump(); });
        }
    }
}
