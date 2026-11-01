import type { AppConfig } from '../types/config';
import { sourceEnabled } from '../utils/musicSources';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Sparkles, Plus, X, Play, Shuffle, RefreshCw, Pencil, Trash2, LoaderCircle, Folder, Server, Check } from 'lucide-react';
import { smartPlaylistsService, smartError } from '../services/smartPlaylists';
import { playlistsService, isDuplicatePlaylistNameError } from '../services/playlists';
import type { Playlist } from '../types/playlist';
import { plexService } from '../services/plex';
import { audioService } from '../services/audio';
import { beginPlaybackIntent, isPlaybackSuperseded, playbackFailure } from '../utils/playbackIntent';
import { defaultSmartRules, smartOptions, smartRulesValid, smartTemplate } from '../utils/smartPlaylists';
import { smartLocalGenreOptions } from '../utils/localGenreFamilies';
import type { LocalGenreFamily } from '../utils/localGenreFamilies';
import type { SmartPlaylist, SmartRules, SmartSources, SmartAlbum, SmartPreview, SmartSort } from '../types/smartPlaylist';
import type { PlexLibrary } from '../types/plex';
const button = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 text-xs font-semibold cursor-pointer hover:bg-[var(--theme-surface-hover,#292929)] disabled:opacity-40 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';
const primary = button + ' bg-[var(--theme-control-accent,#E5A00D)] text-[color:var(--theme-control-on-accent,#000)]';
const input = 'w-full min-w-0 rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] p-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';
export function SmartPlaylists({ config, isPlaybackAvailable, onCount, onSelectionSaved }: {
    config?: Pick<AppConfig,'sources'>;
    isPlaybackAvailable: boolean;
    onCount: (count: number) => void;
    onSelectionSaved?: (playlist: Playlist) => void;
}) {
    const { t } = useTranslation();
    const [items, setItems] = useState<SmartPlaylist[]>([]);
    const [sources, setSources] = useState<SmartSources | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [dialog, setDialog] = useState<{
        item: SmartPlaylist | null;
    } | null>(null);
    const [retry, setRetry] = useState(0);
    const sourcePreferences = JSON.stringify(config?.sources);
    const visibleItems = items.filter(item => !config || sourceEnabled(config, item.rules.source.kind));
    useEffect(() => { let live = true; setLoading(true); void Promise.all([smartPlaylistsService.list(), smartPlaylistsService.sources()]).then(([list, source]) => { if (live) {
        setItems(list);
        setSources(source);
        onCount(list.filter(item => !config || sourceEnabled(config, item.rules.source.kind)).length);
        setError(null);
    } }).catch(e => { if (live)
        setError(smartError(e)); }).finally(() => { if (live)
        setLoading(false); }); return () => { live = false; }; }, [retry, onCount, sourcePreferences]);
    return <section className="mb-8" aria-label={t('smart.title')}>
  <div className="mb-4 flex flex-wrap items-center justify-between gap-3"><div><h3 className="flex items-center gap-2 text-sm font-bold"><Sparkles size={17} className="text-[color:var(--theme-fg-accent,#E5A00D)]"/>{t('smart.title')} <span className="text-[color:var(--theme-fg-muted,#888)]">({visibleItems.length})</span></h3><p className="mt-1 text-xs text-[color:var(--theme-fg-muted,#888)]">{t('smart.subtitle')}</p></div>
  <button type="button" className={primary} disabled={!sources || !sources.local && !sources.plex_server_id && !sources.navidrome_account_id} onClick={() => setDialog({ item: null })}><Plus size={15}/>{t('smart.create')}</button></div>
  {loading && <p role="status" className="text-xs">{t('smart.loading')}</p>}
  {error && <p role="alert" className="text-sm text-[color:var(--theme-fg-error,#ff9999)]">{t(`smart.errors.${error}`)} <button className={button} onClick={() => setRetry(v => v + 1)}>{t('smart.retry')}</button></p>}
  {!loading && !error && visibleItems.length === 0 && <div className="rounded-xl border border-dashed border-[color:var(--theme-border-soft,#333)] p-5 text-xs text-[color:var(--theme-fg-muted,#888)]">{sources && !sources.local && !sources.plex_server_id && !sources.navidrome_account_id ? t('smart.noSources') : t('smart.empty')}</div>}
  <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">{visibleItems.map(item => <button key={item.id} type="button" onClick={() => setDialog({ item })} className="group rounded-xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] p-5 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)]">
   <div className="flex items-center justify-between gap-2"><Sparkles size={23} className="text-[color:var(--theme-fg-accent,#E5A00D)]"/><span className="rounded-full bg-[var(--theme-surface-raised,#252525)] px-2.5 py-1 text-[10px] font-bold uppercase">{t(`smart.sources.${item.rules.source.kind}`)}</span></div>
   <h4 className="mt-3 truncate font-bold">{item.name}</h4><p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#888)]">{t('smart.upTo', { count: item.rules.limit })} · {t(`smart.sort.${item.rules.sort}`)}</p></button>)}</div>
  {dialog && sources && (!dialog.item || !config || sourceEnabled(config, dialog.item.rules.source.kind)) && <SmartPlaylistDialog key={dialog.item?.id ?? 'new'} item={dialog.item} sources={sources} isPlaybackAvailable={isPlaybackAvailable} onSelectionSaved={playlist => { onSelectionSaved?.(playlist); setDialog(null); }} onClose={() => setDialog(null)} onSaved={list => { setItems(list); onCount(list.length); setDialog(null); }}/>}
  <h3 className="mt-8 border-t border-[color:var(--theme-border-soft,#292929)] pt-6 text-sm font-bold">{t('smart.manual')}</h3>
 </section>;
}
export function SmartPlaylistDialog({ item, sources, isPlaybackAvailable, onClose, onSaved, onSelectionSaved }: {
    item: SmartPlaylist | null;
    sources: SmartSources;
    isPlaybackAvailable: boolean;
    onClose: () => void;
    onSaved: (items: SmartPlaylist[]) => void;
    onSelectionSaved?: (playlist: Playlist) => void;
}) {
    const { t } = useTranslation();
    const [name, setName] = useState(item?.name ?? '');
    const [rules, setRules] = useState<SmartRules>(() => item?.rules ?? defaultSmartRules(sources.local ? { kind: 'local' } : sources.plex_server_id ? { kind: 'plex', server_id: sources.plex_server_id, library_id: '' } : {kind:'navidrome',account_id:sources.navidrome_account_id ?? ''}));
    const [editing, setEditing] = useState(!item);
    const [exactGenre, setExactGenre] = useState(!!item?.rules.genre);
    const [albums, setAlbums] = useState<SmartAlbum[]>([]);
    const [libraries, setLibraries] = useState<PlexLibrary[]>([]);
    const [catalogLoading, setCatalogLoading] = useState(false);
    const [libraryLoading, setLibraryLoading] = useState(false);
    const [catalogError, setCatalogError] = useState<string | null>(null);
    const [catalogRetry, setCatalogRetry] = useState(0);
    const [preview, setPreview] = useState<{
        key: string;
        value: SmartPreview;
    } | null>(null);
    const [previewBusy, setPreviewBusy] = useState(false);
    const [saving, setSaving] = useState(false);
    const [selectionName, setSelectionName] = useState<string | null>(null);
    const [playing, setPlaying] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const mounted = useRef(true);
    const liveKey = useRef('');
    const pending = useRef(false);
    const request = useRef(0);
    const playIntent = useRef<ReturnType<typeof beginPlaybackIntent> | null>(null);
    const playPending = useRef(false);
    const frame = useRef<HTMLDivElement>(null);
    const closeRef = useRef(onClose);
    closeRef.current = onClose;
    const key = JSON.stringify(rules);
    liveKey.current = key;
    const currentPreview = preview?.key === key ? preview.value : null;
    const sourceKey = JSON.stringify(rules.source);
    const genres = useMemo(() => smartOptions(albums, 'genres'), [albums]);
    const genreFamilies = useMemo(() => smartLocalGenreOptions(albums).map(value => ({ value, label: t(`localGenreFamilies.${value}`) })).sort((a, b) => a.value === 'other' ? 1 : b.value === 'other' ? -1 : a.label.localeCompare(b.label)), [albums, t]);
    const groupedGenre = rules.source.kind === 'local' && !exactGenre;
    const artists = useMemo(() => smartOptions(albums, 'artist'), [albums]);
    const change = (next: SmartRules) => { liveKey.current = JSON.stringify(next); setRules(next); setSelectionName(null); setError(null); };
    useEffect(() => { mounted.current = true; const previous = document.activeElement; frame.current?.querySelector<HTMLElement>('button,input,select')?.focus(); const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') {
        event.stopPropagation();
        event.preventDefault();
        if (!pending.current)
            closeRef.current();
    } if (event.key === 'Tab') {
        const all = Array.from(frame.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex="0"]') ?? []);
        const first = all[0], last = all[all.length - 1];
        if (!first) {
            event.preventDefault();
        }
        else if (event.shiftKey && document.activeElement === first) {
            event.preventDefault();
            last.focus();
        }
        else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first.focus();
        }
    } }; document.addEventListener('keydown', escape); return () => { mounted.current = false; request.current++; playIntent.current?.cancel(); document.removeEventListener('keydown', escape); if (previous instanceof HTMLElement)
        previous.focus(); }; }, []);
    useEffect(() => { if (!editing || rules.source.kind !== 'plex')
        return; let live = true; setLibraryLoading(true); void plexService.getLibraries().then(list => { if (!live)
        return; setLibraries(list); setRules(current => current.source.kind === 'plex' && !current.source.library_id && list.length ? { ...current, source: { ...current.source, library_id: list[0].key } } : current); }).catch(() => { if (live)
        setCatalogError('source'); }).finally(() => { if (live)
        setLibraryLoading(false); }); return () => { live = false; }; }, [editing, rules.source.kind, rules.source.kind === 'plex' ? rules.source.server_id : '', catalogRetry]);
    useEffect(() => { if (!editing)
        return; let live = true; setAlbums([]); setCatalogError(null); if (rules.source.kind === 'plex' && !rules.source.library_id)
        return; setCatalogLoading(true); void smartPlaylistsService.catalog(rules.source, () => live).then(result => { if (live)
        setAlbums(result); }).catch(e => { if (live)
        setCatalogError(smartError(e)); }).finally(() => { if (live)
        setCatalogLoading(false); }); return () => { live = false; }; }, [sourceKey, editing, catalogRetry]);
    const loadPreview = async () => { if (previewBusy || !smartRulesValid(rules))
        return; const own = ++request.current; const ownKey = key; setPreview(null); setSelectionName(null); setPreviewBusy(true); setError(null); try {
        const value = await smartPlaylistsService.preview(rules, () => mounted.current && request.current === own && liveKey.current === ownKey);
        if (mounted.current && request.current === own && liveKey.current === ownKey)
            setPreview({ key: ownKey, value });
    }
    catch (e) {
        if (mounted.current && request.current === own && liveKey.current === ownKey)
            setError(smartError(e));
    }
    finally {
        if (mounted.current && request.current === own)
            setPreviewBusy(false);
    } };
    useEffect(() => { if (item)
        void loadPreview(); }, []);
    const save = async () => { if (pending.current || !currentPreview || !smartRulesValid(rules) || !name.trim())
        return; pending.current = true; setSaving(true); setError(null); try {
        const list = await smartPlaylistsService.save(item?.id ?? null, name.trim(), rules);
        if (mounted.current)
            onSaved(list);
    }
    catch (e) {
        if (mounted.current)
            setError(smartError(e));
    }
    finally {
        pending.current = false;
        if (mounted.current)
            setSaving(false);
    } };
    const saveSelection = async () => {
        if (pending.current || !currentPreview?.tracks.length || !selectionName?.trim()) return;
        pending.current = true; setSaving(true); setError(null);
        // Freeze exactly this visible preview; no fresh selection, rule write,
        // playback request, tag modification or automatic duplicate overwrite.
        const items = currentPreview.tracks.map(track => ({
            media_locator: track.media_locator!,
            metadata: { title: track.title, artist: track.artist, album: track.album, duration: track.duration },
        }));
        try {
            const created = await playlistsService.createWithItems(selectionName.trim(), items);
            if (mounted.current) { onSelectionSaved?.(created); onClose(); }
        } catch (e) {
            if (mounted.current) setError(isDuplicatePlaylistNameError(e) ? 'selectionDuplicate' : 'selectionSave');
        } finally {
            pending.current = false;
            if (mounted.current) setSaving(false);
        }
    };
    const remove = async () => { if (!item || pending.current || !window.confirm(t('playlists.deleteConfirm', { name: item.name })))
        return; pending.current = true; setSaving(true); try {
        const list = await smartPlaylistsService.delete(item.id);
        if (mounted.current)
            onSaved(list);
    }
    catch (e) {
        if (mounted.current)
            setError(smartError(e));
    }
    finally {
        pending.current = false;
        if (mounted.current)
            setSaving(false);
    } };
    const play = async (index = 0, shuffle = false) => { if (!currentPreview?.tracks.length || playPending.current || !isPlaybackAvailable)
        return; playPending.current = true; let tracks = [...currentPreview.tracks]; if (shuffle) {
        for (let i = tracks.length - 1; i > 0; i--) {
            const j = Math.floor(Math.random() * (i + 1));
            [tracks[i], tracks[j]] = [tracks[j], tracks[i]];
        }
        index = 0;
    } const intent = beginPlaybackIntent(); playIntent.current = intent; setPlaying(true); setError(null); try {
        await audioService.playTracks(tracks, index, intent);
    }
    catch (e) {
        if (!isPlaybackSuperseded(e) && await intent.confirmCurrent() && mounted.current)
            setError('play_' + playbackFailure(e));
    }
    finally {
        playPending.current = false;
        if (playIntent.current === intent)
            playIntent.current = null;
        if (mounted.current)
            setPlaying(false);
    } };
    const source = (kind: 'local' | 'plex' | 'navidrome') => { setExactGenre(false); change(defaultSmartRules(kind === 'local' ? { kind: 'local' } : kind === 'plex' ? { kind: 'plex', server_id: sources.plex_server_id ?? '', library_id: '' } : {kind:'navidrome',account_id:sources.navidrome_account_id ?? ''})); };
    const number = (value: string) => value === '' ? null : Number(value);
    return <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm" role="dialog" aria-modal="true" aria-label={item?.name ?? t('smart.create')} onMouseDown={e => { if (e.target === e.currentTarget && !saving)
        onClose(); }}>
 <div ref={frame} className="flex max-h-[90vh] w-full max-w-5xl flex-col overflow-hidden rounded-2xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] text-[color:var(--theme-fg-primary,#fff)] shadow-2xl">
  <header className="flex items-center justify-between gap-3 border-b border-[color:var(--theme-border-soft,#333)] px-6 py-4"><div><p className="flex items-center gap-2 text-xs font-semibold uppercase tracking-widest text-[color:var(--theme-fg-accent,#E5A00D)]"><Sparkles size={15}/>{t('smart.title')}</p><h2 className="mt-1 text-xl font-bold">{item?.name ?? t('smart.create')}</h2></div><button type="button" className={button} aria-label={t('playlists.cancel')} disabled={saving} onClick={onClose}><X size={18}/></button></header>
  <div className="min-h-0 flex-1 overflow-y-auto p-6">
   <p className="mb-5 text-xs leading-relaxed text-[color:var(--theme-fg-muted,#999)]">{t('smart.rulesHint')}</p>
   {editing ? <>
    <div className="mb-5 flex flex-wrap gap-2">{['custom', 'recent', 'favorites', 'genre', 'decade'].map(template => <button type="button" key={template} className={button} disabled={saving || catalogLoading} onClick={() => { setExactGenre(false); change(smartTemplate(template, rules, albums)); if (!item)
            setName(t(`smart.templates.${template}`)); }}>{t(`smart.templates.${template}`)}</button>)}</div>
    {rules.source.kind === 'navidrome' && sources.navidrome_account_id && rules.source.account_id !== sources.navidrome_account_id && <div className="mb-4 flex flex-wrap items-center gap-3 text-xs"><span>{t('smart.errors.source')}</span><button type="button" className={button} disabled={saving} onClick={() => source('navidrome')}>{t('smart.useCurrentServer')}</button></div>}
    {rules.source.kind === 'plex' && sources.plex_server_id && rules.source.server_id !== sources.plex_server_id && <div className="mb-4 flex flex-wrap items-center gap-3 text-xs"><span>{t('smart.errors.source')}</span><button type="button" className={button} disabled={saving} onClick={() => source('plex')}>{t('smart.useCurrentServer')}</button></div>}
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
     <label className="space-y-1 text-xs"><span>{t('smart.name')}</span><input className={input} value={name} maxLength={100} disabled={saving} onChange={e => setName(e.target.value)}/></label>
     <label className="space-y-1 text-xs"><span>{t('smart.source')}</span><select className={input} value={rules.source.kind} disabled={saving} onChange={e => source(e.target.value as 'local' | 'plex' | 'navidrome')}>{sources.local && <option value="local">{t('smart.sources.local')}</option>}{sources.plex_server_id && <option value="plex">{t('smart.sources.plex')}</option>}{sources.navidrome_account_id && <option value="navidrome">{t('smart.sources.navidrome')}</option>}</select></label>
     {rules.source.kind === 'plex' && <label className="space-y-1 text-xs"><span>{t('smart.library')}</span><select className={input} value={rules.source.library_id} disabled={saving} onChange={e => { if (rules.source.kind === 'plex')
            change({ ...rules, source: { ...rules.source, library_id: e.target.value } }); }}><option value="">{t('smart.choose')}</option>{libraries.map(l => <option key={l.key} value={l.key}>{l.title}</option>)}</select></label>}
     <div className="space-y-1 text-xs"><label className="block space-y-1"><span>{t('smart.genre')}</span><select className={input} value={(groupedGenre ? rules.genre_family : rules.genre) ?? ''} disabled={saving || catalogLoading} onChange={e => change(groupedGenre ? { ...rules, genre: null, genre_family: (e.target.value || null) as LocalGenreFamily | null } : { ...rules, genre: e.target.value || null, genre_family: null })}><option value="">{t('smart.any')}</option>{groupedGenre ? <>{rules.genre_family && !genreFamilies.some(f => f.value === rules.genre_family) && <option value={rules.genre_family}>{t(`localGenreFamilies.${rules.genre_family}`)}</option>}{genreFamilies.map(family => <option key={family.value} value={family.value}>{family.label}</option>)}</> : <>{rules.genre && !genres.includes(rules.genre) && <option>{rules.genre}</option>}{genres.map(value => <option key={value}>{value}</option>)}</>}</select></label>{rules.source.kind === 'local' && <><p className="text-[color:var(--theme-fg-muted,#888)]">{t(groupedGenre ? 'smart.genreFamilyHint' : 'smart.genreTagHint')}</p><button type="button" className="cursor-pointer text-[color:var(--theme-fg-accent,#E5A00D)] underline underline-offset-2 disabled:opacity-40" disabled={saving} onClick={() => { setExactGenre(v => !v); change({ ...rules, genre: null, genre_family: null }); }}>{t(groupedGenre ? 'smart.useOriginalTag' : 'smart.useGenreFamily')}</button></>}</div>
     <label className="space-y-1 text-xs"><span>{t('smart.artist')}</span><select className={input} value={rules.artist ?? ''} disabled={saving || catalogLoading} onChange={e => change({ ...rules, artist: e.target.value || null })}><option value="">{t('smart.any')}</option>{rules.artist && !artists.includes(rules.artist) && <option>{rules.artist}</option>}{artists.map(value => <option key={value}>{value}</option>)}</select></label>
     <div className="grid grid-cols-2 gap-2"><label className="space-y-1 text-xs"><span>{t('smart.yearFrom')}</span><input type="number" min={1000} max={9999} className={input} value={rules.year_from ?? ''} disabled={saving} onChange={e => change({ ...rules, year_from: number(e.target.value) })}/></label><label className="space-y-1 text-xs"><span>{t('smart.yearTo')}</span><input type="number" min={1000} max={9999} className={input} value={rules.year_to ?? ''} disabled={saving} onChange={e => change({ ...rules, year_to: number(e.target.value) })}/></label></div>
     <label className="space-y-1 text-xs"><span>{t('smart.added')}</span><select className={input} value={rules.added_days ?? ''} disabled={saving} onChange={e => change({ ...rules, added_days: number(e.target.value) })}><option value="">{t('smart.anyTime')}</option>{[7, 30, 90, 180, 365].map(days => <option key={days} value={days}>{t('smart.days', { count: days })}</option>)}</select></label>
     <label className="space-y-1 text-xs"><span>{t('smart.order')}</span><select className={input} value={rules.sort} disabled={saving} onChange={e => change({ ...rules, sort: e.target.value as SmartSort })}>{['album', 'artist', 'added_newest', 'added_oldest', 'year_newest', 'year_oldest'].map(mode => <option key={mode} value={mode}>{t(`smart.sort.${mode}`)}</option>)}</select></label>
     <label className="space-y-1 text-xs"><span>{t('smart.trackMode')}</span><select className={input} value={rules.track_mode ?? 'album'} disabled={saving} onChange={e => change({ ...rules, track_mode: e.target.value as 'varied' | 'album' })}><option value="varied">{t('smart.variedTracks')}</option><option value="album">{t('smart.albumTracks')}</option></select></label>
     <label className="space-y-1 text-xs"><span>{t('smart.limit')}</span><input type="number" min={1} max={500} className={input} value={rules.limit} disabled={saving} onChange={e => change({ ...rules, limit: Number(e.target.value) })}/></label>
    </div>
    <label className="my-4 flex items-center gap-2 text-sm"><input type="checkbox" checked={rules.favorites_only} disabled={saving} onChange={e => change({ ...rules, favorites_only: e.target.checked })}/>{t('smart.favorites')}</label>
    <p className="mb-3 text-xs text-[color:var(--theme-fg-muted,#888)]">{t('smart.datesHint')}</p>
   </> : <div className="mb-5 flex flex-wrap items-center gap-2 text-xs"><span className={button}>{rules.source.kind === 'local' ? <Folder size={14}/> : <Server size={14}/>} {t(`smart.sources.${rules.source.kind}`)}</span>{[rules.genre_family ? t(`localGenreFamilies.${rules.genre_family}`) : rules.genre, rules.artist, rules.year_from || rules.year_to ? `${rules.year_from ?? '…'}–${rules.year_to ?? '…'}` : null, rules.added_days ? t('smart.days', { count: rules.added_days }) : null, rules.favorites_only ? t('smart.favorites') : null, t('smart.upTo', { count: rules.limit }), t(rules.track_mode === 'varied' ? 'smart.variedTracks' : 'smart.albumTracks'), t(`smart.sort.${rules.sort}`)].filter(Boolean).map((value, i) => <span className="rounded-full border border-[color:var(--theme-border-soft,#333)] px-3 py-1.5" key={i}>{value}</span>)}</div>}
   {(catalogLoading || libraryLoading) && <p role="status" className="mb-3 flex items-center gap-2 text-xs"><LoaderCircle size={14} className="animate-spin"/>{t('smart.catalogLoading')}</p>}
   {catalogError && editing && <p role="alert" className="mb-3 text-xs text-[color:var(--theme-fg-error,#ff9999)]">{t(`smart.errors.${catalogError}`)} <button type="button" className={button} onClick={() => setCatalogRetry(v => v + 1)}>{t('smart.retry')}</button></p>}
   {!smartRulesValid(rules) && <p role="status" className="mb-3 text-xs text-[color:var(--theme-fg-error,#ff9999)]">{t('smart.errors.invalid')}</p>}
   {error && <p role="alert" className="mb-3 text-sm text-[color:var(--theme-fg-error,#ff9999)]">{error.startsWith('play_') ? t(error === 'play_failed' ? 'playlists.playError' : `player.requestError.${error.slice(5)}`) : t(`smart.errors.${error}`)}</p>}
   {selectionName !== null && currentPreview && <form onSubmit={e => { e.preventDefault(); void saveSelection(); }} className="mb-4 rounded-xl border border-[color:var(--theme-border-soft,#333)] p-4"><p className="mb-3 text-xs text-[color:var(--theme-fg-muted,#999)]">{t('smart.selectionHint')}</p><div className="flex flex-wrap items-end gap-2"><label className="min-w-0 flex-1 space-y-1 text-xs"><span>{t('smart.selectionName')}</span><input autoFocus className={input} value={selectionName} maxLength={100} disabled={saving} onChange={e => setSelectionName(e.target.value)}/></label><button type="submit" className={primary} disabled={saving || !selectionName.trim()}>{t('smart.confirmSaveSelection')}</button><button type="button" className={button} disabled={saving} onClick={() => setSelectionName(null)}>{t('playlists.cancel')}</button></div></form>}
   <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[color:var(--theme-border-soft,#333)] py-4"><button type="button" className={button} disabled={previewBusy || saving || !smartRulesValid(rules)} onClick={() => void loadPreview()}>{previewBusy ? <LoaderCircle size={15} className="animate-spin"/> : <RefreshCw size={15}/>} {t(previewBusy ? 'smart.previewLoading' : editing ? 'smart.preview' : 'smart.refresh')}</button>
   {currentPreview && <strong className="text-sm" role="status">{t('smart.previewCount', { count: currentPreview.tracks.length, albums: currentPreview.matched_albums })}{currentPreview.limit_reached && <span className="ml-2 text-xs font-normal text-[color:var(--theme-fg-muted,#888)]">{t('smart.limitReached')}</span>}</strong>}</div>
   {!currentPreview && !previewBusy && <p className="py-6 text-center text-sm text-[color:var(--theme-fg-muted,#888)]">{t('smart.previewFirst')}</p>}
   {currentPreview && <div className="rounded-xl border border-[color:var(--theme-border-soft,#333)]">{currentPreview.tracks.length === 0 ? <p className="p-6 text-center text-sm">{t('smart.noMatches')}</p> : currentPreview.tracks.map((track, index) => <div key={index} className="flex items-center gap-3 border-b border-[color:var(--theme-border-soft,#292929)] px-4 py-2.5 last:border-0"><span className="w-6 text-right text-xs text-[color:var(--theme-fg-muted,#888)]">{index + 1}</span><div className="min-w-0 flex-1"><p className="truncate text-sm font-semibold">{track.title}</p><p className="truncate text-xs text-[color:var(--theme-fg-muted,#888)]">{track.artist} · {track.album}</p></div><button type="button" className={button} aria-label={t('playlists.playFromTrack')} disabled={!isPlaybackAvailable || playing || saving} onClick={() => void play(index)}><Play size={14}/></button></div>)}</div>}
  </div>
  <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-[color:var(--theme-border-soft,#333)] px-6 py-4"><div className="flex gap-2">{item && <><button type="button" className={button} disabled={saving || playing} onClick={() => { if (editing) {
        setExactGenre(!!item.rules.genre);
        change(item.rules);
        setName(item.name);
    } setEditing(v => !v); }}><Pencil size={14}/>{t(editing ? 'smart.view' : 'smart.edit')}</button><button type="button" className={button} disabled={saving || playing} onClick={() => void remove()}><Trash2 size={14}/>{t('playlists.delete')}</button></>}</div><div className="flex flex-wrap gap-2"><button type="button" className={button} disabled={saving || playing || !currentPreview?.tracks.length} onClick={() => setSelectionName(name)}><Plus size={14}/>{t('smart.saveSelection')}</button>{editing && <button type="button" className={primary} disabled={saving || !currentPreview || !name.trim() || !smartRulesValid(rules)} onClick={() => void save()}>{saving ? <LoaderCircle size={14} className="animate-spin"/> : <Check size={14}/>} {t('smart.save')}</button>}<button type="button" className={button} disabled={!isPlaybackAvailable || !currentPreview?.tracks.length || playing || saving} onClick={() => void play(0, true)}><Shuffle size={14}/>{t('playlists.shuffle')}</button><button type="button" className={primary} disabled={!isPlaybackAvailable || !currentPreview?.tracks.length || playing || saving} onClick={() => void play()}>{playing ? <LoaderCircle size={14} className="animate-spin"/> : <Play size={14}/>} {t('playlists.play')}</button></div></footer>
 </div></div>;
}
