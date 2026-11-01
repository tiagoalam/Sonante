import type { NavidromeAlbum, NavidromePage } from '../types/navidrome';

export interface NavidromeFeedState {
    albums: readonly NavidromeAlbum[];
    more: boolean;
    loading: boolean;
    error: unknown | null;
    total: number | null;
}
export const emptyNavidromeFeed = (): NavidromeFeedState => ({ albums: [], more: true, loading: false, error: null, total: null });

/** One query/account per feed. Retiring it fences every outstanding reply. */
export class NavidromeAlbumFeed {
    private live = true;
    private offset = 0;
    private refreshPending = false;
    private ids = new Set<string>();
    private state = emptyNavidromeFeed();
    private fetchPage: (offset: number) => Promise<NavidromePage>;
    private publish: (state: NavidromeFeedState) => void;
    constructor(fetchPage: (offset: number) => Promise<NavidromePage>, publish: (state: NavidromeFeedState) => void) {
        this.fetchPage = fetchPage; this.publish = publish;
    }

    async refresh(): Promise<void> {
        if (!this.live || this.state.loading) return;
        this.refreshPending = true;
        await this.loadMore();
    }
    async loadMore(): Promise<void> {
        if (!this.live || this.state.loading || (!this.state.more && !this.refreshPending)) return;
        const replacing = this.refreshPending;
        const offset = replacing ? 0 : this.offset;
        this.update({ ...this.state, loading: true, error: null });
        try {
            const page = await this.fetchPage(offset);
            if (!this.live) return;
            if (page.offset !== offset || page.albums.length > 72) throw 'NAVIDROME_PROTOCOL';
            const seen = new Set(replacing ? [] : this.ids);
            const added = page.albums.filter(album => {
                if (seen.has(album.id)) return false;
                seen.add(album.id); return true;
            });
            // A server ignoring offsets must not cause an endless background download.
            if (page.more && page.albums.length > 0 && !added.length) throw 'NAVIDROME_PROTOCOL';
            this.offset = offset + page.albums.length;
            this.ids = seen;
            this.refreshPending = false;
            const albums = replacing ? added : [...this.state.albums, ...added];
            this.update({ albums, more: page.more && page.albums.length > 0, loading: false, error: null, total: typeof page.total === 'number' && Number.isSafeInteger(page.total) && page.total >= 0 ? page.total : (replacing ? null : this.state.total) ?? (!page.more ? albums.length : null) });
        } catch (error) {
            if (this.live) this.update({ ...this.state, loading: false, error });
        }
    }
    dispose(): void { this.live = false; }
    private update(state: NavidromeFeedState): void { this.state = state; this.publish(state); }
}
