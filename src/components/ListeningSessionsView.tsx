import { BetaBadge, ExperimentalNotice } from './ExperimentalFeatures';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { Headphones, Plus, Play, RefreshCw, Trash2, ArrowUp, ArrowDown, Check, LoaderCircle, Disc3 } from 'lucide-react';
import type { AppConfig } from '../types/config';
import type { PlaybackStatus, TrackMetadata } from '../types/audio';
import type { SmartSource, SmartSources } from '../types/smartPlaylist';
import type { ListeningSession, SessionAlbum, SessionCandidate, SessionMode } from '../types/listeningSession';
import type { PlexLibrary } from '../types/plex';
import { sourceEnabled } from '../utils/musicSources';
import { smartPlaylistsService } from '../services/smartPlaylists';
import { sessionCatalog, sessionAlbumTracks, checkSessionSource, sessionLocalAlbum } from '../services/listeningSessions';
import { plexService } from '../services/plex';
import { historyService } from '../services/history';
import { audioService } from '../services/audio';
import { navidromeService } from '../services/navidrome';
import { navidromeAlbumCover } from '../utils/navidromeArtwork';
import { localAlbumCatalog } from '../utils/localAlbumCatalogSession';
import { localArtworkResolver } from '../utils/localArtworkSession';
import { sessionUsesGenreFamilies, sessionGenreValues, sessionMatchesGenre } from '../utils/listeningSessionGenres';
import { beginPlaybackIntent, isPlaybackSuperseded } from '../utils/playbackIntent';
import { listeningSessionStore } from '../utils/listeningSessionStore';
import { bindSessionProgress, suspendSessionProgress, getSessionPlayback, subscribeSessionPlayback } from '../utils/listeningSessionProgress';
import { orderedCandidates, prepareSessionAlbums, sessionAlbum, sessionTrackKey, SESSION_EVENT } from '../utils/listeningSessions';
import { resolveSessionSelection, sameSessionSelection, sessionDisplayName } from '../utils/listeningSessionState';
import { TrackArtwork } from './TrackArtwork';
import { SessionAlbumChoices } from './SessionAlbumChoices';
import { sessionPosition, sessionReplacementCandidates, prepareSessionReplacement } from '../utils/listeningSessionPolish';
const button = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 text-xs font-semibold hover:bg-[var(--theme-surface-hover,#292929)] disabled:opacity-40 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';
const primary = button + ' bg-[var(--theme-control-accent,#E5A00D)] text-[color:var(--theme-control-on-accent,#000)]';
const input = 'w-full rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2.5 text-sm';
const minutes = (seconds: number) => Math.round(seconds / 60);
function SessionCover({ album, candidate, source }: { album: Pick<SessionAlbum, 'id' | 'title'>; candidate?: SessionCandidate; source: SmartSource }) {
  const [thumb, setThumb] = useState<string | null>(null);
  useEffect(() => {
    let live = true; setThumb(null);
    void (async () => {
      await checkSessionSource(source, () => live);
      if (source.kind === 'local') {
        const value = sessionLocalAlbum(album.id) ?? localAlbumCatalog.getSnapshot().albums.find(a => a.id === album.id);
        if (value) { const cover = await localArtworkResolver.resolveLocalOnly(value); if (live) setThumb(cover); }
      } else if (source.kind === 'navidrome') {
        const status = await navidromeService.status();
        if (!live || !status.connection_id || status.account_id !== source.account_id) return;
        const value = await navidromeService.album(status.connection_id, album.id);
        if (live && value.album.cover_id) { const cover = await navidromeAlbumCover(status.connection_id, value.album.cover_id, () => live); if (live) setThumb(cover); }
      }
    })().catch(() => {});
    return () => { live = false; };
  }, [album.id, JSON.stringify(source)]);
  return <TrackArtwork track={{ thumb, plex_image: candidate?.image }} alt={album.title} />;
}
export function ListeningSessionsView({ config, available, active, status }: { config: AppConfig; available: boolean; active: boolean; status: PlaybackStatus }) {
  const { t } = useTranslation();
  const [saved, setSaved] = useState<ListeningSession[]>([]), [session, setSession] = useState<ListeningSession | null>(null);
  const [sources, setSources] = useState<SmartSources | null>(null), [source, setSource] = useState<SmartSource | null>(null);
  const [libraries, setLibraries] = useState<PlexLibrary[]>([]), [catalog, setCatalog] = useState<SessionCandidate[]>([]);
  const [scope, setScope] = useState(''), [genre, setGenre] = useState(''), [mode, setMode] = useState<SessionMode>('discovery');
  const [target, setTarget] = useState(3600), [name, setName] = useState('');
  const [loading, setLoading] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0), [deleteId, setDeleteId] = useState<string | null>(null);
  const [replacement, setReplacement] = useState<{ context: object; sessionId: string; albumId: string; candidates: SessionCandidate[]; page: number } | null>(null);
  const replacementReturn = useRef<HTMLButtonElement | null>(null), albumRows = useRef<HTMLOListElement>(null);
  const life = useRef(0), operation = useRef(false), alive = useRef(false), intentRef = useRef<ReturnType<typeof beginPlaybackIntent> | null>(null);
  const live = useRef({ active, config, source }); live.current = { active, config, source };
  const titleRef = useRef<HTMLHeadingElement>(null);
  const sourceKey = JSON.stringify(source), preferenceKey = JSON.stringify(config.sources);
  useEffect(() => {
    alive.current = true; titleRef.current?.focus();
    const reload = () => { try { setSaved(listeningSessionStore.list()); } catch { setError('storage'); } };
    reload(); window.addEventListener(SESSION_EVENT, reload); window.addEventListener('storage', reload);
    return () => { alive.current = false; life.current++; intentRef.current?.cancel(); window.removeEventListener(SESSION_EVENT, reload); window.removeEventListener('storage', reload); };
  }, []);
  useEffect(() => { if (!active) { life.current++; intentRef.current?.cancel(); } }, [active]);
  useEffect(() => {
    let current = true;
    void smartPlaylistsService.sources().then(async value => {
      if (!current) return; setSources(value);
      if (!source || !sourceEnabled(config, source.kind)) setSource(value.local ? { kind: 'local' } : value.plex_server_id
        ? { kind: 'plex', server_id: value.plex_server_id, library_id: '' } : value.navidrome_account_id ? { kind: 'navidrome', account_id: value.navidrome_account_id } : null);
      if (value.plex_server_id && sourceEnabled(config, 'plex')) { const libs = await plexService.getLibraries(); if (current) setLibraries(libs); }
    }).catch(() => { if (current) setError('source'); });
    return () => { current = false; };
  }, [preferenceKey, retry]);
  useEffect(() => {
    if (source?.kind === 'plex' && !source.library_id && libraries[0]) setSource({ ...source, library_id: libraries[0].key });
  }, [sourceKey, libraries]);
  useEffect(() => {
    const own = ++life.current; setLoading(false); setCatalog([]); setScope(''); setGenre('');
    if (!source || !sourceEnabled(config, source.kind) || source.kind === 'plex' && !source.library_id || !active) return;
    setLoading(true); setError(null);
    const current = () => alive.current && own === life.current && live.current.active;
    void sessionCatalog(source, current).then(value => { if (current()) setCatalog(value); })
      .catch(() => { if (current()) setError('read'); }).finally(() => { if (own === life.current && alive.current) setLoading(false); });
    return () => { life.current++; };
  }, [sourceKey, preferenceKey, active, retry]);
  const choices = (['local', 'plex', 'navidrome'] as const).filter(kind => sourceEnabled(config, kind) && (kind === 'local' ? sources?.local : kind === 'plex' ? sources?.plex_server_id : sources?.navidrome_account_id));
  const scopes = useMemo(() => [...new Map(catalog.filter(a => a.scope_id).map(a => [a.scope_id!, a.scope_label ?? a.scope_id!])).entries()], [catalog]);
  const genres = useMemo(() => sessionGenreValues(catalog, source).map(value => ({ value, label: sessionUsesGenreFamilies(source) ? t(`localGenreFamilies.${value}`) : value })), [source?.kind, catalog, t]);
  const pool = useMemo(() => catalog.filter(a => (!scope || a.scope_id === scope) && sessionMatchesGenre(a, source, genre)), [catalog, scope, genre, source?.kind]);
  const playback = useSyncExternalStore(subscribeSessionPlayback, getSessionPlayback);
  const playingSession = useMemo(() => resolveSessionSelection(playback, saved, playback), [playback, saved]);
  const selected = useMemo(() => resolveSessionSelection(session, saved, playback), [session, saved, playback]);
  const selectedIsPlaying = sameSessionSelection(selected, playback);
  const visible = saved.filter(item => sourceEnabled(config, item.source.kind));
  const replacementContext = useMemo(() => ({}), [selected?.id, sourceKey, pool, mode, active, preferenceKey]);
  const replacementLive = useRef(replacementContext); replacementLive.current = replacementContext;
  const shownReplacement = active && selected && replacement?.sessionId === selected.id && replacement.context === replacementContext ? replacement : null;
  useEffect(() => { setReplacement(null); }, [replacementContext]);
  const changeSource = (kind: string) => {
    life.current++; setSession(null); setError(null);
    setSource(kind === 'local' ? { kind: 'local' } : kind === 'plex' && sources?.plex_server_id ? { kind: 'plex', server_id: sources.plex_server_id, library_id: libraries[0]?.key ?? '' }
      : sources?.navidrome_account_id ? { kind: 'navidrome', account_id: sources.navidrome_account_id } : null);
  };
  const run = async (work: (current: () => boolean) => Promise<void>) => {
    if (operation.current || !active) return;
    operation.current = true; const own = ++life.current, deadline = Date.now() + 120000; setBusy(true); setError(null);
    const current = () => alive.current && live.current.active && life.current === own && Date.now() <= deadline;
    try { await work(current); } catch (e) { if (alive.current && live.current.active && life.current === own && !isPlaybackSuperseded(e) && (e !== 'SESSION_CANCELLED' || Date.now() > deadline)) setError(Date.now() > deadline ? 'timeout' : e === 'SESSION_EMPTY' ? 'empty' : e === 'SESSION_CHANGED' ? 'changed' : e === 'SESSION_STORAGE' ? 'storage' : e === 'SESSION_HISTORY' ? 'history' : 'read'); }
    finally { operation.current = false; if (alive.current) setBusy(false); }
  };
  const prepare = () => { if (!source) return; void run(async current => {
    let heard = new Set<string>();
    if (mode === 'unplayed') { const history = await historyService.list(); if (!current()) return; if (!history.enabled) throw 'SESSION_HISTORY'; heard = new Set(history.entries.map(entry => sessionTrackKey(entry.track.media_locator)).filter((key): key is string => !!key)); }
    const result = await prepareSessionAlbums(orderedCandidates(pool, mode), source, target, mode, a => sessionAlbumTracks(source, a, current), current, heard);
    if (current()) {
      const name = sessionDisplayName('', t('sessions.suggestedName', { album: result.albums[0].title, artist: result.albums[0].artist }), saved);
      setSession({ id: crypto.randomUUID(), name, created_at: Date.now(), source, target, albums: result.albums, cursor: 0, completed: false }); setName(name);
    }
  }); };
  const persist = (value: ListeningSession) => {
    try {
      const named = { ...value, name: sessionDisplayName(name, value.name || t('sessions.defaultName'), saved, value.id) };
      listeningSessionStore.save(named); setSession(named); setName(named.name); setError(null);
      if (named.completed && sameSessionSelection(named, playback)) suspendSessionProgress();
    }
    catch { setError('storage'); }
  };
  const changeAlbums = (albums: SessionAlbum[]) => { if (selected) { const edited = { ...selected, id: crypto.randomUUID(), created_at: Date.now(), albums, cursor: 0, completed: false, started_at: undefined }; setSession(edited); } };
  const openReplacement = (index: number, trigger: HTMLButtonElement) => {
    if (!selected || busy || !active || sourceKey !== JSON.stringify(selected.source)) return;
    const candidates = sessionReplacementCandidates(pool, selected, mode);
    if (!candidates.length || !selected.albums[index]) return;
    replacementReturn.current = trigger; setError(null);
    setReplacement({ context: replacementContext, sessionId: selected.id, albumId: selected.albums[index].id, candidates, page: 0 });
  };
  const closeReplacement = () => {
    if (busy) return; setReplacement(null);
    requestAnimationFrame(() => { if (live.current.active && replacementReturn.current?.isConnected) replacementReturn.current.focus(); });
  };
  const chooseReplacement = (candidate: SessionCandidate) => {
    const choice = shownReplacement;
    if (!selected || !choice || busy || !choice.candidates.includes(candidate)) return;
    void run(async current => {
      const valid = () => current() && replacementLive.current === choice.context;
      const albums = await prepareSessionReplacement(selected, choice.albumId, candidate,
        value => sessionAlbumTracks(selected.source, value, valid), valid);
      if (valid()) {
        const index = selected.albums.findIndex(album => album.id === choice.albumId);
        changeAlbums(albums); setReplacement(null);
        requestAnimationFrame(() => { if (live.current.active) albumRows.current?.querySelector<HTMLElement>(`[data-session-slot="${index}"] h3`)?.focus(); });
      }
    });
  };
  const play = () => { if (!selected || !available || !sourceEnabled(config, selected.source.kind)) return; void run(async current => {
    const intent = beginPlaybackIntent(); intentRef.current = intent;
    const valid = () => current() && intent.current() && sourceEnabled(live.current.config, selected.source.kind);
    const batches: TrackMetadata[][] = [];
    for (const album of selected.albums) {
      const tracks = await sessionAlbumTracks(selected.source, catalog.find(a => a.id === album.id) ?? { ...album, genres: [], added_at: null }, valid);
      if (JSON.stringify(sessionAlbum({ ...album, genres: [], added_at: null }, tracks, selected.source, album.reason).keys) !== JSON.stringify(album.keys)) throw 'SESSION_CHANGED';
      batches.push(tracks);
    }
    if (!valid()) return;
    const value = { ...selected, started_at: Date.now(), name: name.trim() || selected.name, cursor: selected.completed ? 0 : selected.cursor, completed: false };
    suspendSessionProgress();
    await audioService.playTracks(batches.flat(), value.cursor, intent);
    if (valid()) { setSession(value); try { await bindSessionProgress(value, valid); } catch { if (valid()) setError('progress'); } }
  }); };
  const progress = useMemo(() => sessionPosition(selected), [selected]);
  const position = progress?.albumIndex ?? 0;
  const duration = selected?.albums.every(a => a.duration !== null) ? selected.albums.reduce((sum, a) => sum + a.duration!, 0) : null;
  return <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto bg-[var(--theme-surface-base,#121212)]">
    <header className="border-b border-[color:var(--theme-border-soft,#292929)] px-8 py-6"><h1 ref={titleRef} tabIndex={-1} className="flex items-center gap-3 text-2xl font-bold outline-none"><Headphones className="text-[color:var(--theme-fg-accent,#E5A00D)]" />{t('sessions.title')}<BetaBadge /></h1><p className="mt-2 max-w-2xl text-sm text-[color:var(--theme-fg-muted,#999)]">{t('sessions.subtitle')}</p><ExperimentalNotice /></header>
    <div className="space-y-7 p-8">
      {playingSession && !selectedIsPlaying && sourceEnabled(config, playingSession.source.kind) && <section aria-label={t('sessions.currentSession')} className="flex flex-wrap items-center justify-between gap-4 rounded-xl border border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-panel,#181818)] p-4">
        <div><p className="text-xs font-semibold text-[color:var(--theme-fg-accent,#E5A00D)]">{t(status.state === 'play' ? 'sessions.listeningNow' : 'sessions.currentSession')}</p><h2 className="mt-1 font-semibold">{playingSession.name}</h2>
          {sessionPosition(playingSession) && <p className="mt-1 text-xs text-[color:var(--theme-fg-muted,#999)]">{t('sessions.position', sessionPosition(playingSession)!)}</p>}</div>
        <button className={button} disabled={busy} onClick={() => { setSession(playingSession); setName(playingSession.name); setSource(playingSession.source); setError(null); }}>{t('sessions.showCurrent')}</button>
      </section>}
      {error && <div role="alert" className="flex flex-wrap items-center gap-3 rounded-xl border border-[color:var(--theme-border-soft,#444)] p-4 text-sm text-[color:var(--theme-fg-error,#ff9999)]">{t(`sessions.errors.${error}`)}<button className={button} disabled={busy} onClick={() => { setError(null); setRetry(v => v + 1); }}>{t('smart.retry')}</button></div>}
      <section className="rounded-2xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] p-5"><h2 className="mb-4 text-lg font-semibold">{t('sessions.prepare')}</h2>
        <fieldset disabled={busy || !active} className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
          <label className="space-y-2 text-xs">{t('smart.source')}<select className={input} value={source?.kind ?? ''} onChange={e => changeSource(e.target.value)}>{!choices.length && <option value="">{t('smart.noSources')}</option>}{choices.map(kind => <option key={kind} value={kind}>{t(`smart.sources.${kind}`)}</option>)}</select></label>
          {source?.kind === 'plex' ? <label className="space-y-2 text-xs">{t('sessions.library')}<select className={input} value={source.library_id} onChange={e => { setSession(null); setSource({ ...source, library_id: e.target.value }); }}>{libraries.map(lib => <option key={lib.key} value={lib.key}>{lib.title}</option>)}</select></label> : <label className="space-y-2 text-xs">{t('sessions.library')}<select className={input} value={scope} onChange={e => setScope(e.target.value)}><option value="">{t('sessions.allLibraries')}</option>{scopes.map(([id, label]) => <option key={id} value={id}>{label}</option>)}</select></label>}
          <label className="space-y-2 text-xs">{t(sessionUsesGenreFamilies(source) ? 'sessions.genreFamily' : 'smart.genre')}<select className={input} value={genre} onChange={e => setGenre(e.target.value)}><option value="">{t('sessions.anyGenre')}</option>{genres.map(g => <option key={g.value} value={g.value}>{g.label}</option>)}</select></label>
          <label className="space-y-2 text-xs">{t('sessions.priority')}<select className={input} value={mode} onChange={e => setMode(e.target.value as SessionMode)}>{(['discovery', 'recent', 'unplayed'] as const).map(value => <option key={value} value={value}>{t(`sessions.modes.${value}`)}</option>)}</select></label>
        </fieldset>
        <div className="mt-5 flex flex-wrap items-center justify-between gap-4"><div className="flex items-center gap-2" role="group" aria-label={t('sessions.duration')}>{[1800, 3600, 5400].map(value => <button key={value} type="button" className={target === value ? primary : button} aria-pressed={target === value} disabled={busy} onClick={() => setTarget(value)}>{t('sessions.durationOption', { count: minutes(value) })}</button>)}</div><button className={primary} disabled={busy || loading || !pool.length || !source || !active} onClick={prepare}>{busy || loading ? <LoaderCircle size={16} className="animate-spin" /> : <Plus size={16} />}{t(loading ? 'smart.loading' : 'sessions.prepare')}</button></div>
        <p className="mt-3 text-xs text-[color:var(--theme-fg-muted,#888)]">{t('sessions.timeHint')}</p>
        {!loading && source && !catalog.length && !error && <p role="status" className="mt-3 text-sm">{t('sessions.noAlbums')}</p>}
      </section>
      {selected && sourceEnabled(config, selected.source.kind) && <section aria-label={t('sessions.preview')} className="space-y-5">
        <div className="flex flex-wrap items-center justify-between gap-4"><div><p className="mb-1 text-xs font-bold uppercase tracking-widest text-[color:var(--theme-fg-accent,#E5A00D)]">{t('sessions.preview')}</p><input aria-label={t('sessions.name')} maxLength={120} className={input + ' max-w-md font-semibold'} value={name} onChange={e => setName(e.target.value)} disabled={busy} /><p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#999)]">{t('sessions.summary', { count: selected.albums.length })} · {duration === null ? t('sessions.unknownDuration') : t('sessions.minutes', { count: minutes(duration) })}</p></div>
          <div className="flex flex-wrap gap-2"><button className={button} disabled={busy} onClick={() => persist(selected)}><Check size={15} />{t('sessions.save')}</button><button className={primary} disabled={busy || !available || !active} onClick={play}><Play size={16} />{t(selected.started_at && !selected.completed ? 'sessions.resume' : 'sessions.start')}</button></div></div>
        {progress && !selected.completed && <div className="rounded-xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] px-4 py-3">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2"><span className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t(selectedIsPlaying ? status.state === 'play' ? 'sessions.listeningNow' : 'sessions.currentSession' : selected.started_at || selected.cursor > 0 ? 'sessions.savedPosition' : 'sessions.startPosition')}</span><span className="text-sm font-semibold text-[color:var(--theme-fg-accent,#E5A00D)]">{t('sessions.position', progress)}</span></div>
          <div role="progressbar" aria-label={t('sessions.positionLabel')} aria-valuemin={0} aria-valuemax={progress.total} aria-valuenow={progress.position} aria-valuetext={t('sessions.position', progress)} className="h-1.5 overflow-hidden rounded-full bg-[var(--theme-surface-raised,#303030)]"><div className="h-full rounded-full bg-[var(--theme-control-accent,#E5A00D)]" style={{ width: `${progress.position / progress.total * 100}%` }} /></div>
        </div>}
        <p className="text-xs text-[color:var(--theme-fg-muted,#888)]">{t('sessions.resumeHint')}</p>
        <p className="text-xs text-[color:var(--theme-fg-muted,#888)]">{t(saved.some(item => sameSessionSelection(item, selected)) ? 'sessions.explicitlySaved' : 'sessions.unsavedHint')}</p>
        {status.options && (status.options.random || status.options.repeat || status.options.single !== 'off') && <p role="status" className="text-xs text-[color:var(--theme-fg-accent,#E5A00D)]">{t('sessions.playbackModesHint')}</p>}
        <ol ref={albumRows} className="grid gap-4 lg:grid-cols-2">{selected.albums.map((album, index) => <li key={album.id} data-session-slot={index} className={`flex flex-wrap content-start gap-4 rounded-xl border bg-[var(--theme-surface-panel,#181818)] p-4 ${index === position && selectedIsPlaying && !selected.completed ? 'border-[color:var(--theme-border-accent,#E5A00D)]' : 'border-[color:var(--theme-border-soft,#333)]'}`}>
          <div className="h-24 w-24 shrink-0 overflow-hidden rounded-lg"><SessionCover album={album} source={selected.source} candidate={catalog.find(a => a.id === album.id)} /></div><div className="min-w-0 flex-1"><span className="text-[10px] font-bold uppercase tracking-widest text-[color:var(--theme-fg-accent,#E5A00D)]">{t('sessions.step', { index: index + 1 })}</span><h3 tabIndex={-1} className="mt-1 truncate font-bold outline-none" title={album.title}>{album.title}</h3><p className="truncate text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{album.artist}</p><p className="mt-1 text-xs text-[color:var(--theme-fg-muted,#888)]">{album.duration === null ? t('sessions.unknownDuration') : t('sessions.minutes', { count: minutes(album.duration) })} · {t('sessions.tracks', { count: album.keys.length })}</p><p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#999)]">{t(`sessions.reasons.${album.reason}`)}</p>
            <div className="mt-3 flex flex-wrap gap-2"><button className={button} disabled={busy || index === 0} aria-label={t('sessions.up')} onClick={() => { const list = [...selected.albums]; [list[index - 1], list[index]] = [list[index], list[index - 1]]; changeAlbums(list); }}><ArrowUp size={13} /></button><button className={button} disabled={busy || index === selected.albums.length - 1} aria-label={t('sessions.down')} onClick={() => { const list = [...selected.albums]; [list[index + 1], list[index]] = [list[index], list[index + 1]]; changeAlbums(list); }}><ArrowDown size={13} /></button><button className={button} disabled={busy || loading || !active || sourceKey !== JSON.stringify(selected.source) || !pool.some(a => !selected.albums.some(item => item.id === a.id))} onClick={event => openReplacement(index, event.currentTarget)}><RefreshCw size={13} />{t('sessions.replace')}</button><button className={button} disabled={busy || selected.albums.length === 1} aria-label={t('sessions.removeAlbum')} onClick={() => changeAlbums(selected.albums.filter((_, i) => i !== index))}><Trash2 size={13} /></button></div>
          </div>
          {shownReplacement?.albumId === album.id && <SessionAlbumChoices title={album.title}
            candidates={shownReplacement.candidates.slice(shownReplacement.page * 3, shownReplacement.page * 3 + 3)}
            busy={busy} more={shownReplacement.candidates.length > 3} error={error ? t(`sessions.errors.${error}`) : null}
            onChoose={chooseReplacement} onClose={closeReplacement}
            onMore={() => setReplacement(value => value && ({ ...value, page: (value.page + 1) % Math.ceil(value.candidates.length / 3) }))}
            renderArtwork={candidate => <SessionCover album={candidate} candidate={candidate} source={selected.source} />} />}
        </li>)}</ol>
        {saved.some(s => s.id === selected.id) && <button className={button} disabled={busy || selected.completed} onClick={() => persist({ ...selected, completed: true })}><Check size={15} />{t(selected.completed ? 'sessions.completed' : 'sessions.finish')}</button>}
      </section>}
      <section><h2 className="mb-4 text-lg font-semibold">{t('sessions.saved')} <span className="text-[color:var(--theme-fg-muted,#888)]">({visible.length})</span></h2>{!visible.length && <p className="rounded-xl border border-dashed border-[color:var(--theme-border-soft,#333)] p-6 text-sm text-[color:var(--theme-fg-muted,#999)]">{t('sessions.empty')}</p>}<div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">{visible.map(item => <article key={item.id} className="rounded-xl border border-[color:var(--theme-border-soft,#333)] p-4"><button className="w-full text-left" disabled={busy} onClick={() => { setSession(item); setName(item.name); setSource(item.source); setError(null); }}><Disc3 size={22} className="mb-3 text-[color:var(--theme-fg-accent,#E5A00D)]" /><h3 className="truncate font-semibold">{item.name}</h3><p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#999)]">{t(`smart.sources.${item.source.kind}`)} · {t('sessions.summary', { count: item.albums.length })}</p><p className="mt-1 text-xs text-[color:var(--theme-fg-muted,#999)]">{t(item.completed ? 'sessions.completed' : sameSessionSelection(item, playback) ? status.state === 'play' ? 'sessions.listeningNow' : 'sessions.currentSession' : item.started_at ? 'sessions.inProgress' : 'sessions.ready')}</p>{item.started_at && !item.completed && sessionPosition(item) && <p className="mt-2 text-xs text-[color:var(--theme-fg-accent,#E5A00D)]">{t('sessions.position', sessionPosition(item)!)}</p>}</button><button className={button + ' mt-3'} disabled={busy} onClick={() => setDeleteId(item.id)}><Trash2 size={13} />{t('sessions.delete')}</button>{deleteId === item.id && <div className="mt-3 space-y-2 text-xs" role="alert"><p>{t('sessions.confirmDelete')}</p><button className={button} onClick={() => { try { listeningSessionStore.delete(item.id); if (session?.id === item.id) setSession(null); setDeleteId(null); } catch { setError('storage'); } }}>{t('sessions.delete')}</button><button className={button + ' ml-2'} onClick={() => setDeleteId(null)}>{t('settings.cancel')}</button></div>}</article>)}</div></section>
    </div>
    <span className="sr-only" aria-live="polite">{busy ? t('sessions.working') : status.state === 'play' ? t('sessions.audioContinues') : ''}</span>
  </main>;
}
