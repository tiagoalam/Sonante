import { useEffect, useMemo, useRef, useState } from 'react';
import { Headphones, LoaderCircle, Pause, Play, Square, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { AppConfig } from '../types/config';
import type { EditionEntry } from '../utils/albumEditions';
import { editionDisplayTitle, editionTrackName, type EditionProfile } from '../utils/albumEditionAnalysis';
import { abError, abFailure, abStart, type ABFailure, type ABSample, type ABSeconds, type ABSide, type ABTrack } from '../utils/albumAB';
import { AlbumABGraph } from '../utils/albumABGraph';
import { beginPlaybackIntent, playbackIntentToken, subscribePlaybackRequests, type PlaybackIntent } from '../utils/playbackIntent';
import { albumABService as service } from '../services/albumAB';
const control = 'rounded-lg border border-[color:var(--theme-border-soft,#444)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm disabled:opacity-40';
const button = control + ' inline-flex items-center justify-center gap-2 font-semibold hover:bg-[var(--theme-surface-hover,#292929)] focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';
const matchTrack = (title: string, tracks: ABTrack[]) => { const names = tracks.filter(track => editionTrackName(track.title) === editionTrackName(title)); return names.length === 1 ? names[0].key : ''; };
interface Props { entries: readonly EditionEntry[]; profiles: ReadonlyMap<string, EditionProfile | null>; config: AppConfig; active: boolean; context: object }
interface Prepared { token: string; a: ABSample; b: ABSample; scope: object; fields: string }
interface Running { graph: AlbumABGraph; intent: PlaybackIntent; nativeIntent: string | null; token: string; timer?: ReturnType<typeof setInterval>; unsubscribe?: () => void; checking: boolean }
export function AlbumABComparison({ entries, profiles, config, active, context }: Props) {
  const { t } = useTranslation();
  const signature = JSON.stringify(entries.map(entry => entry.key));
  const [versions, setVersions] = useState({ a: entries[0]?.key ?? '', b: entries[1]?.key ?? '' });
  const a = entries.find(entry => entry.key === versions.a) ?? entries[0];
  const b = entries.find(entry => entry.key === versions.b && entry.key !== a?.key) ?? entries.find(entry => entry.key !== a?.key);
  const shared = ['shared', 'pipewire'].includes(config.audio_output_type);
  const supported = a?.source !== 'navidrome' && b?.source !== 'navidrome';
  const [trackRetry, setTrackRetry] = useState(0);
  const scope = useMemo(() => ({}), [context, signature, a?.key, b?.key, active, shared, supported, trackRetry]);
  const [tracks, setTracks] = useState<{ scope: object; a: ABTrack[]; b: ABTrack[]; error: string | null } | null>(null);
  const lists = tracks?.scope === scope ? tracks : null;
  const [choices, setChoices] = useState({ a: '', b: '' });
  const [seconds, setSeconds] = useState<ABSeconds>(30), [start, setStart] = useState('0:00'), [offset, setOffset] = useState(0);
  const [equal, setEqual] = useState(true), [volume, setVolume] = useState(0.6), [side, setSide] = useState<ABSide>('a');
  const [prepared, setPrepared] = useState<Prepared | null>(null), [busy, setBusy] = useState<ABSide | null>(null);
  const [playback, setPlayback] = useState<'idle' | 'starting' | 'playing' | 'paused'>('idle');
  const [position, setPosition] = useState(0), [error, setError] = useState<ABFailure | null>(null), [notice, setNotice] = useState(false);
  const sideRef = useRef(side); sideRef.current = side;
  const fields = JSON.stringify([choices, seconds, start, offset]);
  const live = useRef({ scope, fields, active }); live.current = { scope, fields, active };
  const mounted = useRef(true), operation = useRef(0), token = useRef<string | null>(null), running = useRef<Running | null>(null);
  const stop = (interrupted = false, silent = false) => {
    const session = running.current; running.current = null;
    if (session) {
      session.unsubscribe?.(); if (session.timer) clearInterval(session.timer); session.graph.close(); session.intent.cancel();
      if (session.nativeIntent) void service.end(session.token, session.nativeIntent).catch(() => {});
    }
    if (!silent && mounted.current) { setPlayback('idle'); setPosition(0); setNotice(interrupted); }
  };
  const clear = (silent = false) => {
    operation.current++; stop(false, silent);
    const previous = token.current; token.current = null; if (previous) void service.cancel(previous).catch(() => {});
    if (!silent && mounted.current) { setPrepared(null); setBusy(null); setError(null); }
  };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; clear(true); }; }, []);
  useEffect(() => { clear(); return () => clear(true); }, [scope, fields]);
  useEffect(() => {
    if (!active || !shared || !supported || !a || !b) return;
    let current = true; const valid = () => current && mounted.current && live.current.scope === scope && live.current.active;
    void Promise.all([service.tracks(a, valid), service.tracks(b, valid)]).then(([left, right]) => {
      if (!valid()) return;
      setTracks({ scope, a: left, b: right, error: null });
      setChoices({ a: left[0]?.key ?? '', b: left[0] ? matchTrack(left[0].title, right) : '' });
    }).catch(cause => { if (valid()) setTracks({ scope, a: [], b: [], error: abError(cause) }); });
    return () => { current = false; };
  }, [scope]);
  useEffect(() => { running.current?.graph.settings(equal, volume); }, [equal, volume]);
  useEffect(() => { const hide = () => clear(); const visibility = () => { if (document.hidden) clear(); }; window.addEventListener('pagehide', hide); document.addEventListener('visibilitychange', visibility); return () => { window.removeEventListener('pagehide', hide); document.removeEventListener('visibilitychange', visibility); }; }, []);
  const choose = (which: ABSide, key: string) => {
    setOffset(0); setVersions(previous => which === 'a' ? { a: key, b: key === b?.key ? a?.key ?? previous.a : b?.key ?? previous.b }
      : { b: key, a: key === a?.key ? b?.key ?? previous.b : a?.key ?? previous.a });
  };
  const prepare = async () => {
    const at = abStart(start), bStart = at === null ? null : at + offset / 1000;
    if (!active || !shared || !supported || !a || !b || !lists || !choices.a || !choices.b || at === null || bStart === null || bStart < 0 || bStart > 36_000 || (!Number.isFinite(offset) || Math.abs(offset) > 60000)) { setError({ code: 'invalid' }); return; }
    clear(); const own = operation.current;
    const valid = () => mounted.current && live.current.active && live.current.scope === scope && live.current.fields === fields && operation.current === own;
    let preparingSide: ABSide | undefined;
    setBusy('a'); setNotice(false);
    try {
      const acquired = await service.begin();
      if (!valid()) { void service.cancel(acquired).catch(() => {}); return; } token.current = acquired;
      preparingSide = 'a';
      const left = await service.sample(acquired, 'a', a, choices.a, at, seconds, valid); if (!valid()) return;
      preparingSide = 'b'; setBusy('b'); const right = await service.sample(acquired, 'b', b, choices.b, bStart, seconds, valid); if (!valid()) return;
      setPrepared({ token: acquired, a: left, b: right, scope, fields }); setSide('a'); setBusy(null);
    } catch (cause) { if (valid()) { const previous = token.current; token.current = null; if (previous) void service.cancel(previous).catch(() => {}); setBusy(null); setError(abFailure(cause, preparingSide)); } }
  };
  const play = async () => {
    if (!prepared || prepared.scope !== scope || prepared.fields !== fields || !active) return;
    const old = running.current;
    if (old && playback === 'paused') {
      try { if (!old.nativeIntent || !await service.alive(old.token, old.nativeIntent) || running.current !== old) { if (running.current === old) stop(true); return; } await old.graph.resume(); if (running.current === old) setPlayback('playing'); } catch { if (running.current === old) { stop(); setError({ code: 'unavailable' }); } } return;
    }
    if (old) { try { await old.graph.pause(); if (running.current === old) setPlayback('paused'); } catch { if (running.current === old) { stop(); setError({ code: 'unavailable' }); } } return; }
    setError(null); setNotice(false); setPlayback('starting');
    let session: Running | null = null, pendingContext: AudioContext | null = null;
    try {
      // Called directly from the user gesture, before any IPC await.
      pendingContext = new AudioContext({ sampleRate: 48_000 });
      const graph = new AlbumABGraph(pendingContext, prepared.a, prepared.b, equal, volume);
      pendingContext = null;
      const intent = beginPlaybackIntent(); session = { graph, intent, nativeIntent: null, token: prepared.token, checking: false };
      running.current = session;
      const own = session, valid = () => mounted.current && live.current.active && live.current.scope === scope && live.current.fields === fields && running.current === own && intent.current();
      own.unsubscribe = subscribePlaybackRequests(() => { if (running.current === own) stop(true); });
      await graph.unlock(); const admitted = await playbackIntentToken(intent); if (!valid() || !admitted) { if (running.current === own) stop(); return; }
      own.nativeIntent = admitted; await service.start(own.token, admitted);
      if (!valid() || !await service.alive(own.token, admitted)) { if (running.current === own) stop(true); return; }
      graph.start(sideRef.current); setPlayback('playing');
      own.timer = setInterval(() => {
        if (!valid()) { if (running.current === own) stop(true); return; }
        setPosition(Math.floor(graph.elapsed() * 10) / 10);
        if (own.checking) return; own.checking = true;
        void service.alive(own.token, admitted).then(ok => { if (!ok && valid()) stop(true); }).catch(() => { if (valid()) stop(true); }).finally(() => { own.checking = false; });
      }, 150);
    } catch (cause) { if (pendingContext) void pendingContext.close().catch(() => {}); if (!session || running.current === session) { stop(); if (mounted.current) setError(abFailure(cause)); } }
  };
  const select = (value: ABSide) => { sideRef.current = value; setSide(value); running.current?.graph.select(value); };
  const title = (entry: EditionEntry) => {
    const profile = profiles.get(entry.key), count = profile?.titles?.length ?? profile?.sizes?.length ?? (entry.source === 'local' ? entry.album.track_count : entry.source === 'navidrome' ? entry.album.song_count : null);
    const detail = [editionDisplayTitle(entry, profile), entry.album.year || null, count ? t('editions.trackCount', { count }) : null, entry.origin || null].filter(Boolean).join(' · ');
    return `${entries.indexOf(entry) + 1}. ${detail}`;
  };
  const usable = !!prepared && prepared.scope === scope && prepared.fields === fields;
  const duration = usable ? Math.min(prepared.a.frames, prepared.b.frames) / 48_000 : seconds;
  const format = (value: number) => `${Math.floor(value / 60)}:${Math.floor(value % 60).toString().padStart(2, '0')}`;
  const at = abStart(start), validStart = at !== null && Number.isFinite(offset) && Math.abs(offset) <= 60000 && at + offset / 1000 >= 0 && at + offset / 1000 <= 36_000;
  return <section aria-label={t('ab.title')} tabIndex={0} onKeyDown={event => {
    if (event.code === 'Space') event.stopPropagation();
    if (event.altKey || event.ctrlKey || event.metaKey || (event.target instanceof HTMLElement && /^(INPUT|SELECT|TEXTAREA)$/u.test(event.target.tagName))) return;
    if (['a', 'b'].includes(event.key.toLowerCase())) { event.preventDefault(); event.stopPropagation(); select(event.key.toLowerCase() as ABSide); }
    else if (event.code === 'Space' && usable && !(event.target instanceof HTMLElement && event.target.tagName === 'BUTTON')) { event.preventDefault(); event.stopPropagation(); void play(); }
  }} className="space-y-5 rounded-2xl border border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-panel,#181818)] p-5 outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
    <div><h3 className="flex items-center gap-2 text-lg font-bold"><Headphones size={20} />{t('ab.title')}</h3><p className="mt-2 text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('ab.description')}</p></div>
    {!supported ? <p role="status" className="text-sm">{t('ab.sourcePending')}</p> : !shared ? <p role="status" className="text-sm">{t('ab.sharedOnly')}</p> : <>
      <div className="grid gap-4 md:grid-cols-2">{(['a', 'b'] as const).map(which => {
        const entry = which === 'a' ? a : b, options = which === 'a' ? lists?.a : lists?.b;
        return <div key={which} className="space-y-3 rounded-xl border border-[color:var(--theme-border-soft,#444)] p-4">
          <label className="block"><span className="mb-2 flex items-center gap-2 text-sm font-semibold"><span className="flex h-8 w-8 items-center justify-center rounded-lg bg-[var(--theme-control-accent,#E5A00D)] text-base font-black text-[var(--theme-control-on-accent,#111)]">{which.toUpperCase()}</span>{t('ab.version')}</span>
            <select aria-label={t(`ab.version${which.toUpperCase()}`)} value={entry?.key ?? ''} disabled={!active || !!busy || playback === 'starting'} onChange={event => choose(which, event.target.value)} className={control + ' w-full'}>{entries.map(entry => <option key={entry.key} value={entry.key}>{title(entry)}</option>)}</select></label>
          <label className="block text-sm"><span className="mb-1 block">{t('ab.track')}</span><select aria-label={t(`ab.track${which.toUpperCase()}`)} value={choices[which]} disabled={!lists || !!busy || playback === 'starting'} onChange={event => {
            const key = event.target.value; if (which === 'a') { const track = lists?.a.find(track => track.key === key); setChoices({ a: key, b: track ? matchTrack(track.title, lists?.b ?? []) : '' }); } else setChoices(previous => ({ ...previous, b: key }));
          }} className={control + ' w-full'}><option value="">{t(!lists ? 'ab.loadingTracks' : 'ab.chooseTrack')}</option>{options?.map(track => <option key={track.key} value={track.key}>{track.number}. {track.title}{track.duration ? ` · ${format(track.duration)}` : ''}</option>)}</select></label>
        </div>;
      })}</div>
      {lists?.error && <div role="alert" className="flex flex-wrap items-center gap-3 text-sm text-[color:var(--theme-fg-error,#ff9999)]"><p>{t(`ab.errors.${lists.error}`)}</p><button type="button" className={button} onClick={() => setTrackRetry(value => value + 1)}>{t('ab.retry')}</button></div>}
      <div className="flex flex-wrap items-end gap-5"><fieldset><legend className="mb-2 text-sm font-semibold">{t('ab.sample')}</legend><div className="flex gap-2">{([30, 60, 90] as const).map(value => <button type="button" key={value} aria-pressed={seconds === value} disabled={!!busy || playback === 'starting'} onClick={() => setSeconds(value)} className={button + (seconds === value ? ' border-[color:var(--theme-border-accent,#E5A00D)] text-[color:var(--theme-fg-accent,#E5A00D)]' : '')}>{t('ab.sampleSeconds', { seconds: value })}</button>)}</div></fieldset>
        <label className="block text-sm"><span className="mb-2 block font-semibold">{t('ab.start')}</span><input value={start} onChange={event => setStart(event.target.value)} maxLength={10} placeholder="0:00" disabled={!!busy || playback === 'starting'} aria-invalid={!validStart} className={control + ' w-28 tabular-nums'} /></label>
        <label className="flex items-center gap-2 pb-2 text-sm"><input type="checkbox" checked={equal} onChange={event => setEqual(event.target.checked)} />{t('ab.equalLevel')}</label>
      </div>
      <details className="text-sm text-[color:var(--theme-fg-secondary,#aaa)]"><summary className="cursor-pointer">{t('ab.alignment')}</summary><p className="mt-2 max-w-3xl text-xs">{t('ab.alignmentHint')}</p><label className="mt-3 flex flex-wrap items-center gap-3"><span>{t('ab.offset')}</span><input type="number" value={offset} min={-60000} max={60000} step={10} disabled={!!busy || playback === 'starting'} onChange={event => setOffset(Number(event.target.value))} className={control + ' w-32'} /><span>{t('ab.milliseconds')}</span></label></details>
      {!validStart && <p role="status" className="text-sm text-[color:var(--theme-fg-error,#ff9999)]">{t('ab.errors.invalid')}</p>}
      {error && <p role="alert" className="text-sm text-[color:var(--theme-fg-error,#ff9999)]">{error.side && <strong>{t('ab.failedSample', { side: error.side.toUpperCase() })}{' '}</strong>}{t(`ab.errors.${error.code}`)}{(error.codec || error.sampleRate || error.channels) && <span className="mt-1 block">{t('ab.detectedFormat', { codec: error.codec ?? t('ab.unknownCodec'), rate: error.sampleRate ? t('ab.rate', { rate: error.sampleRate }) : t('ab.unknownRate'), channels: error.channels ? t('ab.channels', { count: error.channels }) : t('ab.unknownChannels') })}</span>}</p>}
      <div className="flex flex-wrap items-center gap-3">{busy ? <><p role="status" className="flex items-center gap-2 text-sm"><LoaderCircle className="animate-spin" size={17} />{t('ab.preparing', { side: busy.toUpperCase() })}</p><button type="button" className={button} onClick={() => clear()}><X size={15} />{t('ab.cancel')}</button></> : <button type="button" className={button} onClick={() => void prepare()} disabled={!active || !lists || !!lists.error || !choices.a || !choices.b || !validStart || playback === 'starting'}>{t(usable ? 'ab.prepareAgain' : 'ab.prepare')}</button>}
        {usable && <button type="button" className={button} disabled={playback === 'starting'} onClick={() => void play()}>{playback === 'starting' ? <LoaderCircle size={16} className="animate-spin" /> : playback === 'playing' ? <Pause size={16} /> : <Play size={16} />}{t(playback === 'playing' ? 'ab.pause' : playback === 'paused' ? 'ab.resume' : playback === 'starting' ? 'ab.starting' : 'ab.listen')}</button>}
        {running.current && <button type="button" className={button} onClick={() => stop()}><Square size={14} />{t('ab.stop')}</button>}
      </div>
      {usable && <div className="space-y-4 rounded-xl bg-[var(--theme-surface-base,#121212)] p-4"><div className="flex gap-3">{(['a', 'b'] as const).map(value => <button type="button" key={value} aria-pressed={side === value} onClick={() => select(value)} className={button + ' min-h-16 flex-1 text-3xl font-black' + (side === value ? ' border-[color:var(--theme-border-accent,#E5A00D)] text-[color:var(--theme-fg-accent,#E5A00D)]' : '')}>{value.toUpperCase()}</button>)}</div>
        <p className="text-center text-sm font-semibold">{t('ab.selected', { side: side.toUpperCase() })}</p><progress className="h-1.5 w-full accent-[var(--theme-fg-accent,#E5A00D)]" max={duration} value={position} aria-label={t('ab.position')} /><div className="flex justify-between text-xs tabular-nums"><span>{format(position)} / {format(duration)}</span><span>{t('ab.loop')}</span></div>
        <label className="flex items-center gap-3 text-sm"><span>{t('ab.volume')}</span><input type="range" min={0} max={100} step={1} value={Math.round(volume * 100)} onChange={event => setVolume(Number(event.target.value) / 100)} aria-label={t('ab.volume')} className="min-w-0 flex-1 accent-[var(--theme-fg-accent,#E5A00D)]" /><span className="w-10 text-right tabular-nums">{Math.round(volume * 100)}%</span></label>
        {duration < seconds - 0.1 && <p className="text-xs">{t('ab.shorter', { duration: format(duration) })}</p>}
      </div>}
      {notice && <p role="status" className="text-sm">{t('ab.interrupted')}</p>}
      <p className="max-w-4xl text-xs leading-relaxed text-[color:var(--theme-fg-muted,#999)]">{t('ab.outputHint')}</p>
    </>}
  </section>;
}
