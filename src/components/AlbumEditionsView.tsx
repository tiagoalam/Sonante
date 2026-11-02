import { BetaBadge, ExperimentalNotice } from './ExperimentalFeatures';
import { AlbumABComparison } from './AlbumABComparison';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, ArrowRight, Layers3, LoaderCircle, Pause, Play, RefreshCw, Search, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { AppConfig } from '../types/config';
import type { PlaybackStatus } from '../types/audio';
import type { SelectedArtist } from '../types/plex';
import type { SourceReadiness } from '../utils/sourceReadiness';
import { sourceEnabled } from '../utils/musicSources';
import { editionWorkKey, filterEditionGroups, groupAlbumEditions, type EditionAnchor, type EditionEntry, type EditionLane, type EditionSource } from '../utils/albumEditions';
import { analyzeEditionGroup, checkEditionCandidates, compareEditionProfiles, editionCandidateKey, editionComparisonEntries, verifiedEditionGroups, type EditionProfile, type VerifiedEditionGroup } from '../utils/albumEditionAnalysis';
import { cachedEditionProfiles, editionArchiveAvailable, restoreEditionProfiles, editionReadScope, invalidateEditionProfiles, readEditionProfile, loadEditionSource } from '../services/albumEditions';
import { LibrarySourceTabs } from './LibrarySourceTabs';
import { EditionCard } from './EditionCard';
import { EditionArtwork } from './EditionArtwork';
import { AlbumView } from './AlbumView';
import { ArtistView } from './ArtistView';
import { LocalBrowserView } from './LocalBrowserView';
import { NavidromeView } from './deferredViews';

const button = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 text-xs font-semibold hover:bg-[var(--theme-surface-hover,#292929)] disabled:opacity-40 focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';
const emptyLanes: EditionLane[] = [];
type ListCheck = { context: object; checked: Set<string>; matches: Map<string, VerifiedEditionGroup[]>; restored: number | null; storageIssue: boolean };
const sources: EditionSource[] = ['local', 'plex', 'navidrome'];
interface AlbumEditionsProps {
  config: AppConfig; available: boolean; active: boolean; status: PlaybackStatus; anchor?: EditionAnchor | null;
  readiness?: Readonly<Record<EditionSource, SourceReadiness>>;
  retrySource?: (source: EditionSource) => void;
  reportSource?: (source: EditionSource, phase: 'ready' | 'error') => void;
  sourceIdentity: object; navidromeRevision: number; catalogRefreshRevision: number;
}
export function AlbumEditionsView(props: AlbumEditionsProps) {
  const { t } = useTranslation();
  const enabled = sources.filter(source => sourceEnabled(props.config, source));
  const [requested, setRequested] = useState<EditionSource>(() => props.anchor?.source ?? enabled[0] ?? 'local');
  const selected = enabled.includes(requested) ? requested : enabled[0];
  const [visited, setVisited] = useState<EditionSource[]>(() => selected ? [selected] : []);
  useEffect(() => {
    if (props.anchor?.source && enabled.includes(props.anchor.source)) setRequested(props.anchor.source);
  }, [props.anchor, props.config.sources]);
  useEffect(() => {
    if (selected && props.active) setVisited(previous => previous.includes(selected) ? previous : [...previous, selected]);
  }, [selected, props.active]);
  return <main data-source-navigation data-album-editions className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]">
    <header className="shrink-0 border-b border-[color:var(--theme-border-soft,#292929)] px-8 py-6"><h1 className="flex items-center gap-3 text-2xl font-bold"><Layers3 className="text-[color:var(--theme-fg-accent,#E5A00D)]" />{t('editions.title')}<BetaBadge /></h1><p className="mt-2 text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('editions.subtitle')}</p><ExperimentalNotice /></header>
    {selected ? <LibrarySourceTabs source={selected} sources={enabled} idPrefix="editions-source" label={t('editions.sourceTabs')}
      labels={{ local: t('smart.sources.local'), plex: t('smart.sources.plex'), navidrome: t('smart.sources.navidrome') }} onChange={source => {
        setVisited(previous => [...new Set([...previous, selected, source])]); setRequested(source);
      }} /> : <p role="status" className="p-8">{t('editions.noSources')}</p>}
    {enabled.filter(source => source === selected || visited.includes(source)).map(source => <section key={source} hidden={selected !== source}
      id={selected === source ? 'editions-source-panel' : undefined} role={selected === source ? 'tabpanel' : undefined} aria-labelledby={selected === source ? `editions-source-${source}` : undefined}
      className={selected === source ? 'flex min-h-0 flex-1 flex-col' : 'hidden'}>
      <SourceAlbumEditionsView {...props} source={source} anchor={!props.anchor?.source || props.anchor.source === source ? props.anchor : null} active={props.active && selected === source} />
    </section>)}
  </main>;
}
export function SourceAlbumEditionsView({ config, available, active, status, anchor = null, sourceIdentity, navidromeRevision, catalogRefreshRevision, source, readiness, retrySource, reportSource }: AlbumEditionsProps & { source: EditionSource }) {
  const { t } = useTranslation();
  const [lanes, setLanes] = useState<EditionLane[]>([]), [retry, setRetry] = useState(0);
  const [profileState, setProfileState] = useState<{ context: object; profiles: Map<string, EditionProfile | null> } | null>(null);
  const [comparison, setComparison] = useState<{ context: object; entries: readonly EditionEntry[] } | null>(null);
  const [query, setQuery] = useState('');
  const [paused, setPaused] = useState(false);
  const [listCheck, setListCheck] = useState<ListCheck | null>(null);
  const listRef = useRef(listCheck); listRef.current = listCheck;
  const [selectedKey, setSelectedKey] = useState<string | null>(() => anchor ? editionWorkKey(anchor) : null);
  const [pageSize, setPageSize] = useState(24), [entryLimit, setEntryLimit] = useState(8);
  const [opened, setOpened] = useState<{ entry: EditionEntry; context: object } | null>(null);
  const [artist, setArtist] = useState<SelectedArtist | null>(null);
  const heading = useRef<HTMLHeadingElement>(null), viewport = useRef<HTMLDivElement>(null), returnButton = useRef<HTMLButtonElement | null>(null);
  const savedScroll = useRef(0), groupHeading = useRef<HTMLHeadingElement>(null), moreMarker = useRef<HTMLDivElement>(null);
  const scope = editionReadScope(config, sourceIdentity, navidromeRevision, catalogRefreshRevision, source);
  const ready = !readiness || readiness[source] === 'ready';
  const context = useMemo(() => ({}), [scope, retry]);
  const live = useRef({ active, context, ready }); live.current = { active, context, ready };
  const [loadedContext, setLoadedContext] = useState<object | null>(null);
  const shown = loadedContext === context ? lanes : emptyLanes;
  const enabled = sourceEnabled(config, source) ? [source] : [];
  const allEntries = useMemo(() => shown.flatMap(lane => lane.entries).filter(entry => entry.source === source), [shown, source]);
  const groups = useMemo(() => groupAlbumEditions(allEntries), [allEntries]);
  // Servers can normalize titles and omit counts. Every repeated work needs a
  // first comparison; cached observations make later visits cheap. Only verified
  // track correspondence AND edition evidence can publish a result.
  const candidates = useMemo(() => groups.filter(group => group.entries.length > 1), [groups]);
  const currentList = listCheck?.context === context ? listCheck : null;
  const families = useMemo(() => [...currentList?.matches.values() ?? []].flat(), [currentList]);
  const suggestions = useMemo(() => {
    const byKey = new Map(families.map(match => [match.group.key, match]));
    return filterEditionGroups(families.map(match => match.group), query, source).map(group => byKey.get(group.key)!);
  }, [families, query, source]);
  const checkedCount = useMemo(() => candidates.filter(group => currentList?.checked.has(editionCandidateKey(group))).length, [candidates, currentList]);
  const pendingChecks = checkedCount < candidates.length;
  const allMatchCount = families.length;
  const cachedProfiles = useMemo(() => cachedEditionProfiles(allEntries, scope), [allEntries, scope, retry, profileState, listCheck, selectedKey, active]);
  const observations = useMemo(() => {
    const profiles = new Map(cachedProfiles);
    for (const match of families) for (const [key, value] of match.profiles) profiles.set(key, value);
    if (profileState?.context === context) for (const [key, value] of profileState.profiles) profiles.set(key, value);
    return profiles;
  }, [cachedProfiles, families, profileState, context]);
  const selectedCandidate = selectedKey ? families.find(match => match.group.key === selectedKey)?.group ?? groups.find(group => group.key === selectedKey) ?? null : null;
  const selectedSignature = selectedCandidate ? JSON.stringify([selectedCandidate.key, selectedCandidate.entries.map(entry => entry.key)]) : '';
  const selected = useMemo(() => selectedCandidate, [context, selectedSignature]);
  const selectedEntries = useMemo(() => selected ? editionComparisonEntries(selected, entryLimit) : [], [selected, entryLimit]);

  const loading = active && ready && (loadedContext !== context || shown.some(lane => lane.phase === 'loading'));
  const detail = opened?.context === context && sourceEnabled(config, opened.entry.source) ? opened.entry : null;
  // Match the exact finite selection, including its catalog generation and batch.
  // Unknown observations are not separate choices while a comparison is pending.
  const comparisonReady = !loading && comparison?.context === context && comparison.entries === selectedEntries;
  const checking = !!selected && !comparisonReady;
  const selectedMatches = useMemo(() => {
    if (!selected || !comparisonReady) return [];
    const matches = verifiedEditionGroups({ ...selected, entries: selectedEntries }, observations);
    return anchor?.album_id && selectedKey === editionWorkKey(anchor) ? matches.filter(match => match.group.entries.some(entry =>
      (entry.source === 'plex' ? entry.album.rating_key : entry.album.id) === anchor.album_id)) : matches;
  }, [selected, selectedEntries, observations, comparisonReady, anchor, selectedKey]);
  const selectedMatch = selectedMatches.length === 1 ? selectedMatches[0] : null;
  const selectedVariants = useMemo(() => selectedMatch ? analyzeEditionGroup(selectedMatch.group, selectedMatch.profiles).variants : [], [selectedMatch]);
  const publishCheck = (group: typeof groups[number], matches: VerifiedEditionGroup[]) => {
    const previous = listRef.current?.context === context ? listRef.current : { context, checked: new Set<string>(), matches: new Map<string, VerifiedEditionGroup[]>(), restored: null, storageIssue: !editionArchiveAvailable(scope) };
    const next = { context, checked: new Set(previous.checked), matches: new Map(previous.matches), restored: previous.restored, storageIssue: previous.storageIssue || !editionArchiveAvailable(scope) }, key = editionCandidateKey(group);
    next.checked.add(key); next.matches.delete(key);
    if (matches.length) next.matches.set(key, matches);
    listRef.current = next; setListCheck(next);
  };
  useEffect(() => { setSelectedKey(anchor ? editionWorkKey(anchor) : null); setOpened(null); setArtist(null); setEntryLimit(8); }, [anchor]);
  useEffect(() => { setOpened(null); setArtist(null); }, [context]);
  useEffect(() => {
    if (!active || !ready || loadedContext === context && lanes.every(lane => lane.phase !== 'loading')) return;
    let current = true;
    setLoadedContext(context);
    setLanes(enabled.map(source => ({ source, phase: 'loading', entries: [] })));
    const valid = () => current && live.current.active && live.current.ready && live.current.context === context;
    for (const source of enabled) {
      void loadEditionSource(source, config, valid).then(lane => {
        if (valid()) { setLanes(previous => previous.map(item => item.source === source ? lane : item)); reportSource?.(source, lane.phase === 'error' ? 'error' : 'ready'); }
      }).catch(() => {
        if (valid()) { setLanes(previous => previous.map(item => item.source === source ? { source, phase: 'error', entries: [] } : item)); reportSource?.(source, 'error'); }
      });
    }
    return () => { current = false; };
  }, [context, active, ready]);
  useEffect(() => {
    if (!active || !ready || shown.some(lane => lane.phase !== 'ready') || loading || selectedKey || loadedContext !== context) return;
    let current = true;
    const valid = () => current && live.current.active && live.current.ready && live.current.context === context;
    void (async () => {
      if (listRef.current?.context !== context || listRef.current.restored === null) {
        const entries = [...new Map(candidates.flatMap(group => editionComparisonEntries(group, 8)).map(entry => [entry.key, entry])).values()];
        const saved = await restoreEditionProfiles(entries, scope, valid);
        if (!valid()) return;
        const restored: ListCheck = { context, checked: new Set(), matches: new Map(), restored: 0, storageIssue: !editionArchiveAvailable(scope) };
        // Publish the saved results together, including discarded copies. Only
        // incomplete/new candidates enter the slower native verification below.
        for (const group of candidates) {
          const entries = editionComparisonEntries(group, 8);
          if (!entries.every(entry => saved.get(entry.key)?.sizes)) continue;
          const key = editionCandidateKey(group); restored.checked.add(key); restored.restored = (restored.restored ?? 0) + 1;
          const matches = verifiedEditionGroups({ ...group, entries }, saved);
          if (matches.length) restored.matches.set(key, matches);
        }
        listRef.current = restored; setListCheck(restored);
      }
      if (paused) return;
      const previous = listRef.current;
      const pending = candidates.filter(group => !previous?.checked.has(editionCandidateKey(group)));
      await checkEditionCandidates(pending, (entry, current) => readEditionProfile(entry, scope, current), valid, publishCheck);
    })().catch(() => { /* Cancellation is fenced; individual failures remain inconclusive. */ });
    return () => { current = false; };
  }, [context, candidates, active, selectedKey, paused, loading, loadedContext, ready]);
  useEffect(() => {
    if (!active || !ready || shown.some(lane => lane.phase !== 'ready') || loading || !selected || loadedContext !== context) return;
    let current = true;
    const profiles = new Map(observations);
    const pending = selectedEntries;
    const valid = () => current && live.current.active && live.current.ready && live.current.context === context;
    setComparison(null);
    void restoreEditionProfiles(pending, scope, valid).then(() => compareEditionProfiles(pending, (entry, valid) => readEditionProfile(entry, scope, valid), valid, (entry, profile) => {
      profiles.set(entry.key, profile);
    })).then(() => {
      if (!valid()) return;
      // Publish one settled result, so cards never merge in front of the user.
      const next = { context, profiles: new Map(profiles) }; setProfileState(next);
      setComparison({ context, entries: selectedEntries });
      // Revalidation can retire a previously shown option after real changes.
      const candidate = groups.find(group => group.key === selected.key || group.entries.some(entry => selected.entries.some(member => member.key === entry.key)));
      if (candidate) {
        publishCheck(candidate, verifiedEditionGroups(candidate, profiles));
      }
    }).catch(() => { /* Obsolete source/detail work cannot publish. */ });
    return () => { current = false; };
  }, [context, selectedEntries, selected, active, loading, loadedContext, ready]);
  useEffect(() => { setPageSize(24); }, [query, source]);
  useEffect(() => {
    const root = viewport.current, marker = moreMarker.current;
    if (!active || selectedKey || !root || !marker || suggestions.length <= pageSize) return;
    let current = true, advanced = false;
    const reveal = () => {
      if (!current || advanced || !live.current.active || live.current.context !== context) return;
      advanced = true; setPageSize(value => Math.min(value + 24, suggestions.length));
    };
    if (typeof IntersectionObserver !== 'undefined') {
      const observer = new IntersectionObserver(rows => { if (rows.some(row => row.isIntersecting)) reveal(); }, { root, rootMargin: '400px 0px' });
      observer.observe(marker);
      return () => { current = false; observer.disconnect(); };
    }
    const onScroll = () => { if (root.scrollHeight - root.scrollTop - root.clientHeight < 400) reveal(); };
    root.addEventListener('scroll', onScroll, { passive: true }); onScroll();
    return () => { current = false; root.removeEventListener('scroll', onScroll); };
  }, [active, context, selectedKey, suggestions.length, pageSize, query, source]);
  useEffect(() => { if (active && selected) groupHeading.current?.focus(); }, [selectedKey, !!selected, active]);
  const closeDetail = () => {
    setOpened(null); setArtist(null);
    requestAnimationFrame(() => { if (!live.current.active) return; if (viewport.current) viewport.current.scrollTop = savedScroll.current; if (returnButton.current?.isConnected) returnButton.current.focus({ preventScroll: true }); else groupHeading.current?.focus(); });
  };
  const open = (entry: EditionEntry, trigger: HTMLButtonElement) => {
    if (!active || !sourceEnabled(config, entry.source) || !allEntries.includes(entry)) return;
    returnButton.current = trigger; savedScroll.current = viewport.current?.scrollTop ?? 0; setArtist(null); setOpened({ entry, context });
  };
  const backToGroups = () => { setSelectedKey(null); setEntryLimit(8); requestAnimationFrame(() => heading.current?.focus()); };
  const refresh = () => { if (!ready) retrySource?.(source); setRetry(value => value + 1); };
  const reanalyze = () => { invalidateEditionProfiles(scope); setPaused(false); setRetry(value => value + 1); };
  const groupRow = (item: VerifiedEditionGroup) => {
    const { group, reasons } = item;
    const cover = group.entries.find(entry => entry.source === 'plex' && entry.album.thumb) ?? group.entries.find(entry => entry.source === 'local') ?? group.entries[0];
    return <button type="button" key={group.key} onClick={() => { setSelectedKey(group.key); setEntryLimit(8); savedScroll.current = 0; if (viewport.current) viewport.current.scrollTop = 0; }}
      className="group flex items-center gap-4 rounded-2xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] p-4 text-left hover:border-[color:var(--theme-border-accent,#E5A00D)] focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
      <span className="h-20 w-20 shrink-0 overflow-hidden rounded-xl"><EditionArtwork entry={cover} active={active} updating={status.is_updating} coverId={observations.get(cover.key)?.cover_id} /></span>
      <span className="min-w-0 flex-1"><span className="block truncate text-base font-bold">{group.title}</span><span className="mt-1 block truncate text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{group.artist}</span>
        <span className="mt-2 block text-xs text-[color:var(--theme-fg-accent,#E5A00D)]">{t(reasons.length ? `editions.reason.${reasons[0]}` : 'editions.checkAvailability')}</span><span className="mt-1 block text-xs text-[color:var(--theme-fg-muted,#999)]">{[...new Set(group.entries.map(entry => entry.source))].map(kind => t(`smart.sources.${kind}`)).join(' · ')}</span></span><ArrowRight size={17} className="shrink-0 opacity-50 group-hover:opacity-100" />
    </button>;
  };
  return <section data-editions-source={source} className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
    <div hidden={!!detail} className={detail ? 'hidden' : 'flex min-h-0 flex-1 flex-col'}>
      <header className="flex shrink-0 flex-wrap items-center justify-end gap-4 border-b border-[color:var(--theme-border-soft,#292929)] px-8 py-6">
        <div className="flex flex-wrap gap-2"><button type="button" className={button} disabled={!active || loading} onClick={refresh}><RefreshCw size={14} />{t('editions.refresh')}</button>
          <button type="button" className={button} disabled={!active || loading || !candidates.length} onClick={reanalyze}>{t('editions.reanalyze')}</button></div>
      </header>
      <div ref={viewport} className="min-h-0 flex-1 space-y-6 overflow-y-auto p-8">
        {readiness && !ready && <div role="status" className="flex flex-wrap items-center gap-3 rounded-xl border border-[color:var(--theme-border-soft,#333)] p-4 text-sm">
          <p>{t(`sourceStatus.${readiness[source]}`)}</p>{readiness[source] === 'error' && <button type="button" className={button} onClick={refresh}>{t('editions.retry')}</button>}
        </div>}
        {!enabled.length && <p role="status">{t('editions.noSources')}</p>}
        {!loading && shown.length > 0 && shown.every(lane => lane.phase === 'unconfigured') && <p role="status">{t('editions.noConfiguredSources')}</p>}
        {shown.some(lane => lane.phase === 'error') && <div role="alert" className="rounded-xl border border-[color:var(--theme-border-soft,#444)] p-4 text-sm"><p>{t('editions.partialHint')}</p><div className="mt-2 flex flex-wrap gap-3">{shown.filter(lane => lane.phase === 'error').map(lane => <span key={lane.source}>{t(`smart.sources.${lane.source}`)}</span>)}<button type="button" className="underline" disabled={loading || !active} onClick={refresh}>{t('editions.retry')}</button></div></div>}
        {loading && <p role="status" className="flex items-center gap-2 text-sm text-[color:var(--theme-fg-muted,#999)]"><LoaderCircle size={15} className="animate-spin" />{t('editions.loading')}</p>}
        {selectedKey ? <section className="space-y-5"><button type="button" className={button} onClick={backToGroups}><ArrowLeft size={14} />{t('editions.allGroups')}</button>
          {selected ? <><div><h2 ref={groupHeading} tabIndex={-1} className="text-3xl font-black outline-none">{selected.title}</h2><p className="mt-1 text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{selected.artist}</p>
            {checking ? <div role="status" aria-live="polite" className="mt-5 flex min-h-32 items-center justify-center gap-3 rounded-2xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] px-5 py-8 text-sm text-[color:var(--theme-fg-secondary,#aaa)]"><LoaderCircle size={18} className="shrink-0 animate-spin" />{t('editions.checkingAlbum')}</div>
              : <p role="status" className="mt-3 text-sm text-[color:var(--theme-fg-muted,#999)]">{t(selectedMatches.length ? 'editions.possibleHint' : 'editions.noComparedEditions')}</p>}</div>
            {comparisonReady && selectedMatches.length > 1 && <div className="grid gap-3 xl:grid-cols-2">{selectedMatches.map(groupRow)}</div>}
            {comparisonReady && selectedMatch && <div className="grid items-stretch gap-5 md:grid-cols-2 2xl:grid-cols-3">{selectedVariants.map(variant => <EditionCard key={variant.entries[0].key} entry={variant.entries[0]} copies={variant.entries} profile={observations.get(variant.entries[0].key)} checked={observations.has(variant.entries[0].key)} active={active} updating={status.is_updating} onOpen={trigger => open(variant.entries[0], trigger)} onOpenCopy={open} />)}</div>}
            {comparisonReady && selectedMatch && selectedVariants.length > 1 && <AlbumABComparison entries={selectedVariants.map(variant => variant.entries[0])} profiles={observations} config={config} active={active && ready && !detail && !artist} context={context} />}
            {entryLimit < selected.entries.length && <button type="button" className={button} disabled={!!checking} onClick={() => setEntryLimit(value => value + 8)}>{t('editions.moreVersions')}</button>}
            <details className="text-xs text-[color:var(--theme-fg-muted,#999)]"><summary className="cursor-pointer">{t('editions.howCompared')}</summary><p className="mt-2 max-w-3xl">{t('editions.matchHint')}</p><p className="mt-2 max-w-3xl">{t('editions.cacheHint')}</p></details>
          </> : !loading && <p role="status" className="text-sm text-[color:var(--theme-fg-muted,#999)]">{t('editions.noOther')}</p>}
        </section> : <section className="space-y-5">
          <div className="flex flex-wrap items-end justify-between gap-4">
            <h2 ref={heading} tabIndex={-1} className="pb-2 text-base font-bold tabular-nums outline-none">{t(query ? 'editions.filteredGroupCount' : 'editions.verifiedGroupCount', { count: suggestions.length, total: allMatchCount })}</h2>
            <div className="flex w-full flex-wrap items-end gap-3 md:w-auto"><label className="block min-w-0 flex-1 md:w-80"><span className="mb-2 block text-sm font-semibold">{t('editions.searchResults')}</span>
              <span className="relative block"><Search size={16} className="pointer-events-none absolute left-3 top-3 text-[color:var(--theme-fg-muted,#888)]" /><input type="search" value={query} maxLength={256} onChange={event => setQuery(event.target.value)} placeholder={t('editions.search')} className="w-full rounded-lg border border-[color:var(--theme-border-soft,#444)] bg-[var(--theme-surface-panel,#181818)] py-2.5 pl-10 pr-3 text-sm outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)]" /></span></label>
              {query && <button type="button" className={button} onClick={() => { setQuery(''); }}><X size={13} />{t('editions.clear')}</button>}</div></div>
          {ready && shown.every(lane => lane.phase === 'ready') && !loading && pendingChecks && candidates.length > 0 && <div role="status" className="rounded-xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)] px-5 py-4 text-sm">
            <div className="flex flex-wrap items-center justify-between gap-3"><p className="flex items-center gap-2 font-semibold">
              {!paused && <LoaderCircle size={15} className="animate-spin" />}
              {t(paused ? 'editions.scanPaused' : !currentList || currentList.restored === null ? 'editions.restoringAnalysis' : 'editions.scanning')}</p>
              <button type="button" className={button} disabled={!active} onClick={() => setPaused(value => !value)}>{paused ? <Play size={13} /> : <Pause size={13} />}{t(paused ? 'editions.resumeAnalysis' : 'editions.pauseAnalysis')}</button></div>
            <div role="progressbar" aria-label={t('editions.scanProgress')} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(100 * checkedCount / candidates.length)} className="mt-3 h-1.5 overflow-hidden rounded-full bg-[var(--theme-surface-hover,#333)]"><div className="h-full rounded-full bg-[var(--theme-fg-accent,#E5A00D)] transition-[width]" style={{ width: `${100 * checkedCount / candidates.length}%` }} /></div>
            <p className="mt-3 max-w-3xl text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{t(currentList?.restored ? 'editions.resumingHint' : 'editions.firstAnalysisHint')}</p>
          </div>}
          {currentList?.storageIssue && <p role="status" className="text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('editions.analysisStorageIssue')}</p>}
          <div className="grid gap-3 xl:grid-cols-2">{suggestions.slice(0, pageSize).map(groupRow)}</div>
          {ready && !loading && !pendingChecks && enabled.length > 0 && !suggestions.length && !shown.every(lane => lane.phase === 'error' || lane.phase === 'unconfigured') && <p role="status" className="rounded-xl border border-dashed border-[color:var(--theme-border-soft,#333)] p-6 text-sm text-[color:var(--theme-fg-muted,#999)]">{t(query ? 'editions.noMatches' : 'editions.empty')}</p>}

          {suggestions.length > pageSize && <div ref={moreMarker} data-editions-more aria-hidden="true" className="h-px" />}
        </section>}
      </div>
    </div>
    {detail?.source === 'local' && <section className="flex min-h-0 flex-1 flex-col"><div className="border-b border-[color:var(--theme-border-soft,#333)] px-8 py-4"><button type="button" className={button} onClick={closeDetail}><ArrowLeft size={14} />{t('editions.back')}</button></div><LocalBrowserView initialAlbumId={detail.album.id} externalBackNavigation onAlbumNavigationBack={closeDetail} albumNavigationAvailable={active} isPlaybackAvailable={available && active} isLibraryUpdating={status.is_updating} catalogRefreshRevision={catalogRefreshRevision} onlineArtworkEnabled={config.online_artwork_enabled} configuredLocalFolders={config.local_folders} /></section>}
    {detail?.source === 'plex' && <>{artist && <ArtistView artist={artist} onBack={() => setArtist(null)} onSelectAlbum={album => { setArtist(null); setOpened({ context, entry: { ...detail, album: { ...album, server_id: detail.serverId, library_id: detail.libraryId }, key: JSON.stringify(['plex', detail.serverId, album.rating_key]) } }); }} status={status} isPlaybackAvailable={available} active={active} requestIdentity={context} />}<div hidden={!!artist} className={artist ? 'hidden' : 'flex min-h-0 flex-1 flex-col'}><AlbumView album={detail.album} active={active && !artist} onBack={closeDetail} backLabel={t('editions.back')} onSelectArtist={setArtist} isPlaybackAvailable={available} /></div></>}
    {detail?.source === 'navidrome' && <NavidromeView embedded initialAlbum={{ account_id: detail.accountId, id: detail.album.id }} active={active} available={available} onAlbumBack={closeDetail} backLabel={t('editions.back')} />}
  </section>;
}
