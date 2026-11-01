import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Disc3, Heart, LoaderCircle, ListPlus, Play, RefreshCw, Search, Server } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { navidromeService, navidromeError } from '../services/navidrome';
import { navidromeAlbumCover as cover } from '../utils/navidromeArtwork';
import { beginPlaybackIntent, isPlaybackSuperseded, playbackFailure } from '../utils/playbackIntent';
import { logError } from '../utils/frontendDiagnostics';
import type { NavidromeStatus, NavidromeAlbum, NavidromeDetail, NavidromeSort } from '../types/navidrome';

import { PlaylistPickerModal } from './PlaylistPickerModal';
import type { NewPlaylistItem } from '../types/playlist';
import { navidromeQueueTrack } from '../utils/navidromeTracks';
import { NavidromeArtistLink } from './NavidromeArtistLink';
import { NavidromeArtistView } from './deferredViews';
import { AlbumCatalog } from './AlbumCatalog';
import { AlbumCatalogRow } from './AlbumCatalogRow';
import { AlbumViewControls } from './AlbumViewControls';
import { ToolbarSelect } from './ToolbarSelect';
import { readAlbumPresentation, saveAlbumPresentation, type AlbumPresentation } from '../utils/albumPresentation';
import { NavidromeAlbumFeed, emptyNavidromeFeed } from '../utils/navidromeAlbumFeed';

const control = 'rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] px-3 py-1.5 text-xs disabled:opacity-40';
export function Artwork({ connection, id }: { connection: string; id: string | null }) {
    const ref = useRef<HTMLDivElement>(null);
    const [near, setNear] = useState(false);
    const [src, setSrc] = useState<string | null>(null);
    useEffect(() => {
        const element = ref.current;
        if (!element) return;
        if (typeof IntersectionObserver === 'undefined') { setNear(true); return; }
        const observer = new IntersectionObserver(entries => {
            if (entries.some(entry => entry.isIntersecting)) { setNear(true); observer.disconnect(); }
        }, { rootMargin: '200px' });
        observer.observe(element);
        return () => observer.disconnect();
    }, []);
    useEffect(() => {
        let live = true;
        setSrc(null);
        if (near && id) void cover(connection, id, () => live).then(value => {
            if (live) setSrc(value);
        }).catch(() => {});
        return () => { live = false; };
    }, [near, connection, id]);
    return <div ref={ref} className="flex aspect-square w-full items-center justify-center overflow-hidden rounded-lg bg-[var(--theme-surface-raised,#202020)] shadow-md">
        {src ? <img alt="" src={src} onError={() => setSrc(null)} className="h-full w-full object-cover transition-transform duration-300 group-hover:scale-105" /> : <Disc3 size={40} className="text-[color:var(--theme-fg-faint,#444)]" />}
    </div>;
}
interface Props {
    available: boolean;
    active?: boolean;
    onConfigure?: () => void;
    initialAlbum?: {account_id:string;id:string};
    onAlbumBack?: () => void;
    backLabel?: string;
    favoritesOnly?: boolean;
    embedded?: boolean;
    onFavoritesChange?: (count: number) => void;
}
export function NavidromeView({ available, active = true, favoritesOnly = false, embedded = false, onFavoritesChange, onConfigure, initialAlbum, onAlbumBack, backLabel }: Props) {
    const { t } = useTranslation();
    const [connection, setConnection] = useState<NavidromeStatus | null>(null);
    const [artistTarget,setArtistTarget] = useState<{account_id:string;id:string}|null>(null);
    const [libraryTotal, setLibraryTotal] = useState<number | null>(null);
    const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);
    const [starting, setStarting] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [feed, setFeed] = useState(emptyNavidromeFeed);
    const feedRef = useRef<NavidromeAlbumFeed | null>(null);
    const [presentation, setPresentation] = useState<AlbumPresentation>(() => readAlbumPresentation('navidrome'));
    const changePresentation = (next: AlbumPresentation) => { setPresentation(next); saveAlbumPresentation('navidrome', next); };
    const [query, setQuery] = useState('');
    const [draftQuery, setDraftQuery] = useState('');
    const [sort, setSort] = useState<NavidromeSort>('recent');
    const [retry, setRetry] = useState(0);
    const [statusRetry, setStatusRetry] = useState(0);
    const [watchReady, setWatchReady] = useState(false);
    const [detail, setDetail] = useState<NavidromeDetail | null>(null);
    const [detailLoading, setDetailLoading] = useState(false);
    const [detailError, setDetailError] = useState<string | null>(null);
    const [albumId, setAlbumId] = useState<string | null>(null);
    const [playing, setPlaying] = useState(false);
    const [favorites, setFavorites] = useState<NavidromeAlbum[]>([]);
    const [favoritesReady, setFavoritesReady] = useState(false);
    const [favoriteError, setFavoriteError] = useState<string | null>(null);
    const [favoriteBusy, setFavoriteBusy] = useState(false);
    const [favoriteRevision, setFavoriteRevision] = useState(0);
    const alive = useRef(true), detailRequest = useRef(0);
    const favoritePending = useRef(false), playIntent = useRef<ReturnType<typeof beginPlaybackIntent> | null>(null);
    const favoriteChanged = useRef(onFavoritesChange);
    favoriteChanged.current = onFavoritesChange;
    const [scrollContainer, setScrollContainer] = useState<HTMLDivElement | null>(null);
    const moreMarker = useRef<HTMLDivElement>(null);
    const catalogPosition = useRef(0);
    const connectionId = connection?.connection_id ?? null;
    const catalogIdentity = connection?.account_id ?? connectionId;
    const displayedCatalogIdentity = useRef(catalogIdentity);
    const catalogCurrent = displayedCatalogIdentity.current === catalogIdentity;
    const albumAccount = useRef<string | null>(null);
    const statusRequest = useRef(0);
    const catalogIdentityRef = useRef(catalogIdentity); catalogIdentityRef.current = catalogIdentity;
    const connectionRef = useRef(connectionId);
    connectionRef.current = connectionId;
    useEffect(() => {
        alive.current = true;
        return () => { alive.current = false; detailRequest.current++; playIntent.current?.cancel(); };
    }, []);
    useEffect(() => {
        let live = true, stop: (() => void) | undefined;
        // Subscribe before the first status snapshot, so connecting in Settings
        // cannot leave a mounted library using the previous proxy/account.
        void navidromeService.subscribeConfiguration(() => {
            if (!live) return;
            statusRequest.current++; detailRequest.current++;
            playIntent.current?.cancel(); playIntent.current = null; setPlaying(false);
            setStarting(true);
            setStatusRetry(value => value + 1);
        }).then(unlisten => {
            if (!live) { unlisten(); return; }
            stop = unlisten; setWatchReady(true);
        }).catch(cause => {
            if (live) { logError('navidrome.catalog', cause); setWatchReady(true); }
        });
        return () => { live = false; stop?.(); };
    }, []);
    useEffect(() => {
        if (!watchReady) return;
        const own = ++statusRequest.current;
        let live = true;
        setStarting(true);
        void navidromeService.status().then(value => {
            if (!live || own !== statusRequest.current) return;
            setConnection(value); setError(null);
        }).catch(cause => { if (live && own === statusRequest.current) setError(navidromeError(cause)); })
            .finally(() => { if (live && own === statusRequest.current) setStarting(false); });
        return () => { live = false; };
    }, [statusRetry, watchReady]);
    useEffect(() => { playIntent.current?.cancel(); playIntent.current = null; setPlaying(false); setPlaylistItems(null); }, [connectionId]);
    useEffect(() => { setLibraryTotal(null); }, [catalogIdentity]);
    useEffect(() => {
        let live = true;
        setFavorites([]); setFavoritesReady(false); setFavoriteError(null);
        if (!connectionId) return;
        void navidromeService.favorites(connectionId).then(albums => {
            if (!live) return;
            setFavorites(albums); setFavoritesReady(true); favoriteChanged.current?.(albums.length);
        }).catch(cause => { if (live) setFavoriteError(navidromeError(cause)); });
        return () => { live = false; };
    }, [connectionId, favoriteRevision]);
    useEffect(() => {
        displayedCatalogIdentity.current = catalogIdentity;
        setFeed(emptyNavidromeFeed()); setError(null);
        setDetail(null); setAlbumId(null); setDetailLoading(false); setDetailError(null); detailRequest.current++; albumAccount.current = null;
        catalogPosition.current = 0;
        scrollContainer?.scrollTo?.({ top: 0 });
        if (!connectionId || favoritesOnly || initialAlbum) return;
        const session = new NavidromeAlbumFeed(
            offset => {
                if (catalogIdentityRef.current !== catalogIdentity || !connectionRef.current) return Promise.reject('NAVIDROME_CONTEXT_CHANGED');
                return navidromeService.albums(connectionRef.current, query, sort, offset, 36);
            },
            state => { setFeed(state); if (!query && state.total !== null) setLibraryTotal(state.total); if (state.error) logError('navidrome.catalog', state.error); },
        );
        feedRef.current = session;
        void session.loadMore();
        return () => { session.dispose(); if (feedRef.current === session) feedRef.current = null; };
        // The scroll element and presentation are deliberately not part of the catalog identity.
    }, [catalogIdentity, query, sort, favoritesOnly, initialAlbum?.id, initialAlbum?.account_id]);
    useEffect(() => {
        if (retry > 0) void feedRef.current?.refresh();
    }, [retry]);
    useEffect(() => {
        const marker = moreMarker.current;
        if (!scrollContainer || !marker || albumId || favoritesOnly || feed.loading || feed.error || !feed.more || typeof IntersectionObserver === 'undefined') return;
        let live = true;
        const observer = new IntersectionObserver(entries => {
            if (live && entries.some(entry => entry.isIntersecting)) void feedRef.current?.loadMore();
        }, { root: scrollContainer, rootMargin: '360px 0px' });
        observer.observe(marker);
        return () => { live = false; observer.disconnect(); };
    }, [scrollContainer, albumId, favoritesOnly, feed.albums.length, feed.loading, feed.error, feed.more]);
    useEffect(() => {
        if (!albumId && scrollContainer) scrollContainer.scrollTop = catalogPosition.current;
    }, [albumId, scrollContainer]);
    async function open(id: string) {
        if (!connectionId || starting || error) return;
        const source = connectionId;
        albumAccount.current = connection?.account_id ?? null;
        const own = ++detailRequest.current;
        if (!albumId) catalogPosition.current = scrollContainer?.scrollTop ?? 0;
        setAlbumId(id); setDetail(null); setDetailError(null); setDetailLoading(true);
        scrollContainer?.scrollTo?.({ top: 0 });
        try {
            const value = await navidromeService.album(connectionId, id);
            if (alive.current && detailRequest.current === own && connectionRef.current === source) setDetail(value);
        } catch (cause) { if (alive.current && detailRequest.current === own && connectionRef.current === source) setDetailError(navidromeError(cause)); }
        finally { if (alive.current && detailRequest.current === own && connectionRef.current === source) setDetailLoading(false); }
    }
    useEffect(() => {
        if (!initialAlbum || !connectionId || starting || error) return;
        if (initialAlbum.account_id !== connection?.account_id) { setAlbumId(initialAlbum.id); setDetailError('changed'); return; }
        void open(initialAlbum.id);
    }, [connectionId, starting, error, initialAlbum?.id, initialAlbum?.account_id]);
    useEffect(() => {
        // A successful same-account reconnect keeps the open destination and
        // scroll/query state; an account change is reset by the feed effect.
        if (!connectionId || starting || error || initialAlbum || !albumId || albumAccount.current !== connection?.account_id) return;
        void open(albumId);
    }, [connectionId, starting]);
    function addToPlaylist(songs: readonly import('../types/navidrome').NavidromeSong[]) {
        if (!connection?.account_id) return;
        setPlaylistItems(songs.map(song => {
            const track = navidromeQueueTrack(connection, song);
            return {media_locator:track.media_locator!,metadata:{title:track.title,artist:track.artist,album:track.album,duration:track.duration}};
        }));
    }
    async function play(index: number, id = detail?.album.id, prepare = false) {
        if (!connectionId || !id || playIntent.current || !available || starting || error) return;
        const source = connectionId;
        const intent = beginPlaybackIntent();
        playIntent.current = intent; setPlaying(true); setDetailError(null);
        try {
            if (prepare) {
                const prepared = await navidromeService.album(connectionId, id);
                if (!intent.current() || playIntent.current !== intent || connectionRef.current !== source) return;
                if (!prepared.songs.length || prepared.songs.length > 500) throw 'NAVIDROME_INVALID';
            }
            if (!intent.current() || playIntent.current !== intent || connectionRef.current !== source) return;
            await navidromeService.play(source, id, index, intent);
        } catch (cause) {
            if (!isPlaybackSuperseded(cause) && await intent.confirmCurrent() && alive.current && playIntent.current === intent && connectionRef.current === source) {
                setDetailError(typeof cause === 'string' && cause.startsWith('NAVIDROME_') ? navidromeError(cause) : `play_${playbackFailure(cause)}`);
                logError('navidrome.play', cause);
            }
        } finally {
            if (playIntent.current === intent) { playIntent.current = null; if (alive.current) setPlaying(false); }
        }
    }
    async function favorite(album: NavidromeAlbum) {
        if (!connectionId || !favoritesReady || favoritePending.current || starting || error) return;
        const own = connectionId;
        favoritePending.current = true; setFavoriteBusy(true); setFavoriteError(null);
        try {
            const albums = await navidromeService.setFavorite(own, album.id, !favorites.some(item => item.id === album.id));
            if (alive.current && connectionRef.current === own) {
                setFavorites(albums); favoriteChanged.current?.(albums.length);
            }
        } catch (cause) { if (alive.current && connectionRef.current === own) setFavoriteError(navidromeError(cause)); }
        finally { favoritePending.current = false; if (alive.current) setFavoriteBusy(false); }
    }
    const isFavorite = (album: NavidromeAlbum) => favorites.some(item => item.id === album.id);
    const favoriteButton = (album: NavidromeAlbum, card = false) => <button type="button" disabled={!favoritesReady || favoriteBusy || !!error}
        aria-pressed={isFavorite(album)} title={t(isFavorite(album) ? 'favorites.removeFavorite' : 'common.addFavorite')}
        aria-label={t(isFavorite(album) ? 'favorites.removeFavorite' : 'common.addFavorite')}
        onClick={event => { event.stopPropagation(); void favorite(album); }}
        className={card ? `absolute right-2 top-2 z-20 rounded-full bg-black/60 p-1.5 shadow transition-colors focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-40 ${isFavorite(album) ? 'text-[#E5A00D]' : 'text-white/80 hover:text-white'}` : control + ' inline-flex items-center gap-2'}>
        <Heart size={card ? 14 : 17} fill={isFavorite(album) ? 'currentColor' : 'none'} />
        {!card && t(isFavorite(album) ? 'favorites.removeFavorite' : 'common.addFavorite')}
    </button>;
    const queryLower = query.toLocaleLowerCase();
    const matchedFavorites = favorites.filter(album => `${album.title}\n${album.artist}`.toLocaleLowerCase().includes(queryLower));
    if (sort === 'name') matchedFavorites.sort((a, b) => a.title.localeCompare(b.title) || a.artist.localeCompare(b.artist));
    const visibleAlbums = favoritesOnly ? matchedFavorites : feed.albums;
    const busy = (!favoritesOnly && feed.loading) || (favoritesOnly && !favoritesReady && !favoriteError);
    const catalogError = !favoritesOnly && feed.error ? navidromeError(feed.error) : null;
    const countLabel = favoritesOnly ? t('favorites.count', { count: matchedFavorites.length }) : libraryTotal === null ? t('navidrome.subtitle') : t('navidrome.libraryCount', { count: libraryTotal });
    const failureNotice = detailError && <p role="alert" className="mb-4 text-sm text-[color:var(--theme-fg-error,#f99)]">{t(detailError.startsWith('play_') ? `player.requestError.${detailError.slice(5)}` : `navidrome.errors.${detailError}`)} {albumId && <button type="button" className={control} disabled={playing} onClick={() => void open(albumId)}>{t('navidrome.retry')}</button>}</p>;
    return <>{artistTarget && <NavidromeArtistView artist={artistTarget} available={available&&!starting&&!error} active={active&&!starting&&!error} revision={statusRetry} onBack={()=>setArtistTarget(null)} />}
    <section hidden={!!artistTarget} className={`${artistTarget ? 'hidden' : 'flex'} min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]`}>
        {!embedded && <header className="flex flex-wrap items-center justify-between gap-3 border-b border-[color:var(--theme-border-soft,#262626)] px-8 py-5">
            <div><h1 className="flex items-center gap-2 text-2xl font-bold"><Server size={23} />{t('navidrome.albumsTitle')}</h1><p className="mt-1 text-sm font-semibold tabular-nums text-[color:var(--theme-fg-secondary,#aaa)]">{connectionId && !starting ? countLabel : t('navidrome.subtitle')}</p></div>

        </header>}
        {(starting || (!!connectionId && !catalogCurrent)) && <p role="status" className="flex items-center gap-2 px-8 py-5"><LoaderCircle size={17} className="animate-spin" />{t('navidrome.loading')}</p>}
        {error && <div role="alert" className="mx-8 my-4 rounded-xl border border-[color:var(--theme-border-soft,#444)] p-4 text-sm"><p>{t(`navidrome.errors.${error}`)}</p><button type="button" className={control + ' mt-3'} disabled={starting} onClick={() => setStatusRetry(value => value + 1)}>{t('navidrome.retry')}</button></div>}
        {!starting && !error && !connectionId && <div className="m-8 max-w-xl space-y-3 rounded-xl border border-[color:var(--theme-border-soft,#333)] p-5"><p>{t('sources.navidromeConfigureHint')}</p><button type="button" className={control} onClick={onConfigure}>{t('sources.configure')}</button></div>}
        {connectionId && !starting && catalogCurrent && <>
            {!albumId && <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-[color:var(--theme-border-soft,#262626)] px-8 py-3">
                <form className="flex min-w-0 flex-wrap gap-2" onSubmit={event => { event.preventDefault(); if (query === draftQuery.trim()) setRetry(value => value + 1); else setQuery(draftQuery.trim()); }}>
                    <div className="relative"><Search size={14} className="absolute left-3 top-2.5 text-[color:var(--theme-fg-muted,#666)]" /><input className={control + ' pl-9'} aria-label={t('navidrome.search')} placeholder={t('navidrome.search')} value={draftQuery} maxLength={256} onChange={event => setDraftQuery(event.target.value)} /></div>
                    <button className={control} type="submit" disabled={busy} aria-label={t('navidrome.search')}><Search size={16} /></button>
                    {query && <button className={control} type="button" onClick={() => { setQuery(''); setDraftQuery(''); }}>{t('navidrome.clearSearch')}</button>}
                </form>
                <div className="flex flex-wrap items-center gap-2">
                    <span className="text-xs text-[color:var(--theme-fg-muted,#888)]">{t('plex.sortLabel')}</span>
                    <ToolbarSelect label={t('navidrome.sort')} className="max-w-52" value={sort}
                        disabled={!!query && !favoritesOnly} onChange={value => setSort(value as NavidromeSort)}
                        options={[{value:'recent',label:t(favoritesOnly ? 'navidrome.favoritesRecent' : 'navidrome.recent')},{value:'name',label:t('navidrome.name')}]} />
                    <AlbumViewControls value={presentation} onChange={changePresentation} />
                    <button className={control} type="button" disabled={busy} aria-label={t('navidrome.refresh')} onClick={() => favoritesOnly ? setFavoriteRevision(value => value + 1) : setRetry(value => value + 1)}><RefreshCw size={16} /></button>
                </div>
            </div>}
            <div ref={setScrollContainer} onScroll={event => {
                const element = event.currentTarget;
                if (!albumId) catalogPosition.current = element.scrollTop;
                if (!albumId && !favoritesOnly && !feed.error && element.scrollHeight - element.scrollTop - element.clientHeight < 360) void feedRef.current?.loadMore();
            }} className="min-h-0 flex-1 overflow-y-auto px-8 py-6">
                {favoriteError && <div role="alert" className="mb-4 text-sm text-[color:var(--theme-fg-error,#f99)]"><p>{t('navidrome.favoritesFailed')} {t(`navidrome.errors.${favoriteError}`)}</p><button type="button" className={control + ' mt-2'} onClick={() => setFavoriteRevision(value => value + 1)}>{t('navidrome.retryFavorites')}</button></div>}
                {albumId ? <>
                    <button type="button" className={control + ' mb-5 inline-flex items-center gap-2'} onClick={() => { if (onAlbumBack) { onAlbumBack(); return; } detailRequest.current++; setAlbumId(null); setDetail(null); setDetailError(null); setDetailLoading(false); }}><ArrowLeft size={16} />{backLabel ?? t('navidrome.back')}</button>
                    {detailLoading && <p role="status">{t('navidrome.loadingAlbum')}</p>}{failureNotice}
                    {detail && <>
                        <div className="mb-7 flex flex-wrap items-end gap-6"><div className="w-52 shrink-0"><Artwork connection={connectionId} id={detail.album.cover_id} /></div><div className="min-w-0 flex-1 space-y-3"><p className="text-xs font-bold uppercase tracking-wider text-[color:var(--theme-fg-accent,#E5A00D)]">{t('navidrome.album')}</p><h2 className="text-3xl font-black">{detail.album.title}</h2><NavidromeArtistLink artist={detail.album.artist} albumId={detail.album.id} connectionId={connectionId} accountId={connection?.account_id??''} onOpen={setArtistTarget} disabled={!active||!!error||!!artistTarget} className="text-base text-[color:var(--theme-fg-secondary,#ccc)]" /><p className="text-xs text-[color:var(--theme-fg-muted,#777)]">{detail.album.year ? `${detail.album.year} · ` : ''}{t('navidrome.tracks', { count: detail.songs.length })}</p><div className="flex flex-wrap gap-3 pt-2"><button type="button" className="inline-flex items-center gap-2 rounded-xl bg-[#E5A00D] px-6 py-2.5 text-xs font-bold text-black disabled:opacity-50" disabled={!available || !!error || playing || !detail.songs.length || detail.songs.length > 500} onClick={() => void play(0)}><Play size={16} fill="currentColor" />{t(playing ? 'navidrome.preparing' : 'navidrome.playAlbum')}</button><button type="button" className={control + ' inline-flex items-center gap-2'} disabled={!connection?.account_id || !detail.songs.length || detail.songs.length > 500} onClick={() => addToPlaylist(detail.songs)}><ListPlus size={16} />{t('playlists.addAlbum')}</button>{favoriteButton(detail.album)}</div></div></div>
                        {detail.songs.length > 500 && <p className="mb-4 text-sm">{t('navidrome.albumLimit')}</p>}
                        <div className="overflow-hidden rounded-xl border border-[color:var(--theme-border-soft,#222)]">
                            <div className="flex gap-4 bg-[var(--theme-surface-panel,#181818)] px-4 py-2.5 text-[11px] font-bold uppercase tracking-wider text-[color:var(--theme-fg-muted,#666)]"><span className="w-8">#</span><span className="flex-1">{t('plex.tracks')}</span></div>
                            {detail.songs.map((song, index) => <div key={song.id} className="flex min-w-0 items-center"><button type="button" disabled={!available || !!error || playing || detail.songs.length > 500} onClick={() => void play(index)} className="group flex min-w-0 flex-1 items-center gap-4 border-t border-[color:var(--theme-border-soft,#222)] px-4 py-3 text-left hover:bg-[var(--theme-surface-hover,#1E1E1E)] disabled:opacity-50"><span className="w-8 text-xs text-[color:var(--theme-fg-muted,#666)]">{song.disc > 1 ? `${song.disc}.` : ''}{song.track || index + 1}</span><span className="min-w-0 flex-1"><span className="block truncate text-xs font-semibold group-hover:text-[color:var(--theme-fg-accent,#E5A00D)]">{song.title}</span><span className="block truncate text-[11px] text-[color:var(--theme-fg-muted,#777)]">{song.artist}</span></span><span className="text-xs text-[color:var(--theme-fg-muted,#888)]">{song.duration ? `${Math.floor(song.duration / 60)}:${Math.floor(song.duration % 60).toString().padStart(2, '0')}` : '—'}</span><Play size={15} /></button><button type="button" className="shrink-0 rounded-lg p-3 text-[color:var(--theme-fg-muted,#888)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] disabled:opacity-40" disabled={!connection?.account_id} aria-label={t('playlists.addTrack')} title={t('playlists.addTrack')} onClick={() => addToPlaylist([song])}><ListPlus size={16} /></button></div>)}
                        </div>
                    </>}
                </> : <>
                    {failureNotice}
                    {embedded && <p className="mb-5 text-sm font-semibold tabular-nums text-[color:var(--theme-fg-secondary,#aaa)]">{countLabel}</p>}
                    {busy && !visibleAlbums.length && <p role="status" className="flex items-center gap-2"><LoaderCircle size={17} className="animate-spin" />{t('navidrome.loading')}</p>}
                    {!busy && !visibleAlbums.length && !error && !catalogError && !favoriteError && (favoritesOnly ? favoritesReady : !feed.more) && <p className="py-10 text-center text-sm text-[color:var(--theme-fg-muted,#999)]">{t(query ? 'navidrome.noResults' : favoritesOnly ? 'navidrome.emptyFavorites' : 'navidrome.empty')}</p>}
                    {!!visibleAlbums.length && <AlbumCatalog items={visibleAlbums} presentation={presentation}
                        getItemKey={album => album.id} scrollContainer={scrollContainer}
                        renderItem={album => presentation.layout !== 'grid' ? <AlbumCatalogRow
                            title={album.title} artist={album.artist} year={album.year ? String(album.year) : undefined} trackCount={album.song_count}
                            detailed={presentation.layout === 'details'}
                            artistAction={<NavidromeArtistLink artist={album.artist} albumId={album.id} connectionId={connectionId} accountId={connection?.account_id??''} onOpen={setArtistTarget} disabled={!active||!!error||!!artistTarget} className="text-xs text-[color:var(--theme-fg-secondary,#aaa)]" />}
                            artwork={presentation.layout === 'list' ? <Artwork connection={connectionId} id={album.cover_id} /> : undefined}
                            favorite={isFavorite(album)} favoriteDisabled={!favoritesReady || favoriteBusy || !!error} readable={!error}
                            playable={available && !error && !playing && (album.song_count ?? 0) <= 500}
                            onOpen={() => void open(album.id)} onPlay={() => void play(0, album.id, true)} onFavorite={() => void favorite(album)} queueAction={null}
                        /> : <div className="group relative flex min-w-0 flex-col">
                            <div className="relative mb-2.5"><button type="button" className="block w-full cursor-pointer" disabled={!!error} aria-label={t('navidrome.openAlbum', { title: album.title })} onClick={() => void open(album.id)}><Artwork connection={connectionId} id={album.cover_id} /></button>{favoriteButton(album, true)}<div className="pointer-events-none absolute inset-0 flex items-center justify-center rounded-lg bg-black/40 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"><button type="button" title={t('navidrome.playAlbum')} aria-label={t('navidrome.quickPlay', { title: album.title })} disabled={!available || !!error || playing || (album.song_count ?? 0) > 500} className="pointer-events-auto flex h-12 w-12 items-center justify-center rounded-full bg-[#E5A00D] text-black shadow-lg disabled:opacity-50" onClick={() => void play(0, album.id, true)}><Play size={20} fill="currentColor" className="ml-1" /></button></div></div>
                            <button type="button" className="truncate text-left text-sm font-semibold" disabled={!!error} title={album.title} onClick={() => void open(album.id)}>{album.title}</button><p className="mt-0.5 truncate text-xs text-[color:var(--theme-fg-secondary,#999)]" title={album.artist}><NavidromeArtistLink artist={album.artist} albumId={album.id} connectionId={connectionId} accountId={connection?.account_id??''} onOpen={setArtistTarget} disabled={!active||!!error||!!artistTarget} /></p><p className="mt-0.5 text-[11px] text-[color:var(--theme-fg-muted,#666)]">{album.year ? `${album.year} · ` : ''}{album.song_count != null ? t('navidrome.tracks', { count: album.song_count }) : ''}</p>
                        </div>}
                    />}
                    {catalogError && <div role="alert" className="my-5 rounded-xl border border-[color:var(--theme-border-soft,#444)] p-4 text-sm"><p>{t(`navidrome.errors.${catalogError}`)}</p><button type="button" className={control + ' mt-3'} onClick={() => void feedRef.current?.loadMore()}>{t('navidrome.retry')}</button></div>}
                    {!favoritesOnly && !error && feed.more && !catalogError && (visibleAlbums.length > 0 || !feed.loading) && <div ref={moreMarker} className="flex min-h-16 items-center justify-center py-5">
                        {feed.loading ? <p role="status" className="flex items-center gap-2 text-xs text-[color:var(--theme-fg-muted,#999)]"><LoaderCircle size={15} className="animate-spin" />{t('navidrome.loading')}</p> : <button type="button" className={control} onClick={() => void feedRef.current?.loadMore()}>{t('navidrome.loadMore')}</button>}
                    </div>}

                </>}
            </div>

        </>}
    </section>{playlistItems && <PlaylistPickerModal items={playlistItems} title={t('playlists.addTrack')} onClose={() => setPlaylistItems(null)} />}</>;
}
