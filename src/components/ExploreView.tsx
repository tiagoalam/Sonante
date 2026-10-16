import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Play, ArrowLeft, Compass, RefreshCw, UserRound, ChevronLeft, ChevronRight, Guitar, Piano, Mic2, Radio, Headphones, Music2, Disc3, AudioLines, ListMusic } from "lucide-react";
import { useTranslation } from "react-i18next";
import { createExploreSession, exploreOriginName, EXPLORE_PAGE_SIZE, EXPLORE_SUGGESTION_COUNT } from "../utils/explore";
import { exploreGenreCardStyle, assignExploreGenreColors, assignExploreGenreIcons } from "../utils/exploreGenreCards";
import { albumShelfEdges, albumShelfLayout, albumShelfTarget } from "../utils/exploreAlbumShelf";
import type { ExploreArtistGroup, ExploreContext, ExploreEntry, ExploreShelfId } from "../utils/explore";
import { artistPortraitInitials } from "../utils/artistInitials";
import { localArtworkResolver } from "../utils/localArtworkSession";
import { ExploreGenreSlice } from "./ExploreGenreSlice";
import { AlbumArtworkPlaceholder } from "./AlbumArtworkPlaceholder";

const genreIcons = { guitar: Guitar, piano: Piano, mic: Mic2, radio: Radio, headphones: Headphones, music: Music2, disc: Disc3, audio: AudioLines, playlist: ListMusic };

export const ExploreAlbumCard: React.FC<{ entry: ExploreEntry; current: () => boolean; revision: number; active: boolean;
  onPlay?: (entry: ExploreEntry, current: () => boolean) => Promise<void>; isPlaybackAvailable?: boolean;
  onlineArtworkEnabled?: boolean; artworkBlocked?: boolean; artworkOnly?: boolean; fallback?: React.ReactNode; onOpen: () => void }> = ({ entry, current, revision, active, onPlay, isPlaybackAvailable = false, onlineArtworkEnabled = false, artworkBlocked = false, artworkOnly = false, fallback, onOpen }) => {
  const { t } = useTranslation();
  const [cover, setCover] = useState<string | null>(() => localArtworkResolver.get(entry.album.id));
  const [phase, setPhase] = useState<"idle" | "loading" | "ready" | "missing" | "error">("idle");
  const [visible, setVisible] = useState(() => typeof IntersectionObserver === "undefined");
  const cardNode = useRef<HTMLElement | null>(null);
  const live = useRef(current); live.current = current;
  const flags = useRef({ active, onlineArtworkEnabled, artworkBlocked });
  flags.current = { active, onlineArtworkEnabled, artworkBlocked };
  const [playing, setPlaying] = useState(false);
  const [playError, setPlayError] = useState(false);
  const pending = useRef(false);
  const playbackEpoch = useRef(0);
  const mounted = useRef(false);
  useLayoutEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useLayoutEffect(() => {
    playbackEpoch.current++;
    return () => { playbackEpoch.current++; };
  }, [entry, revision, active, isPlaybackAvailable, artworkBlocked]);
  const handlePlay = async () => {
    if (pending.current || !active || !isPlaybackAvailable || artworkBlocked || !live.current() || !onPlay) return;
    pending.current = true; setPlaying(true); setPlayError(false);
    const epoch = playbackEpoch.current;
    const valid = () => epoch === playbackEpoch.current && flags.current.active && !flags.current.artworkBlocked && live.current();
    try { await onPlay(entry, valid); }
    catch (error) {
      logError("local.album-play", error);
      if (valid()) setPlayError(true);
    } finally { pending.current = false; if (mounted.current) setPlaying(false); }
  };
  const origin = `${t(`explore.sources.${entry.source}`)} · ${exploreOriginName(entry) ?? t("explore.unknownOrigin")}`;
  useEffect(() => {
    if (!active || typeof IntersectionObserver === "undefined" || !cardNode.current) return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(item => item.isIntersecting)) { setVisible(true); observer.disconnect(); }
    }, { rootMargin: "150px" });
    observer.observe(cardNode.current);
    return () => observer.disconnect();
  }, [entry.key, active]);
  useEffect(() => {
    const cached = localArtworkResolver.get(entry.album.id);
    setCover(cached); setPhase(cached ? "ready" : "idle");
    return localArtworkResolver.subscribe(entry.album.id, value => {
      if (flags.current.active && live.current()) { setCover(value); setPhase("ready"); }
    });
  }, [entry.album, entry.source]);
  useEffect(() => {
    if (!active || !visible || artworkBlocked || !live.current()) return;
    let mounted = true;
    const interested = () => mounted && flags.current.active && !flags.current.artworkBlocked && live.current();
    const cached = localArtworkResolver.get(entry.album.id);
    if (cached) { setCover(cached); setPhase("ready"); return; }
    setPhase("loading");
    let deferred = false;
    void localArtworkResolver.resolve(entry.album, () => flags.current.onlineArtworkEnabled,
      () => flags.current.artworkBlocked, next => { deferred = next === "deferred"; }, interested).then(value => {
      if (interested()) { setCover(value); setPhase(value ? "ready" : deferred ? "idle" : "missing"); }
    }).catch(() => {
      if (interested()) setPhase("error"); // Retry on a later activation, never per render or polling.
    });
    return () => { mounted = false; };
  }, [entry.album, entry.source, active, visible, onlineArtworkEnabled, artworkBlocked]);
  const isAlbumArtwork = artworkOnly && !!cover;
  const artwork = <div aria-busy={phase === "loading"} role={isAlbumArtwork ? "img" : undefined}
      aria-label={isAlbumArtwork ? t("explore.artistAlbumCover", { album: entry.album.title }) : undefined}
      title={isAlbumArtwork ? t("explore.artistAlbumCover", { album: entry.album.title }) : undefined}
      className="aspect-square overflow-hidden rounded-lg bg-[#202020]">
      {cover ? <img src={cover} alt="" className="h-full w-full object-cover" onError={() => { setCover(null); setPhase("error"); }} /> : fallback ?? <AlbumArtworkPlaceholder />}
    </div>;
  if (artworkOnly) return <div ref={cardNode as React.Ref<HTMLDivElement>} className="h-full w-full">{artwork}</div>;
  return <div ref={cardNode as React.Ref<HTMLDivElement>}
    onKeyDown={event => { if (event.key === " " || event.key === "Enter") event.stopPropagation(); }}
    className="group relative w-full min-w-0 rounded-xl p-2 text-left hover:bg-[#222222]">
    <button type="button" onClick={() => { if (active && live.current()) onOpen(); }} disabled={!active}
      aria-label={`${entry.album.title || t("explore.untitled")} · ${entry.album.artist || t("explore.artistNotProvided")}`}
      className="absolute inset-0 rounded-xl focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-60" />
    {artwork}
    {onPlay && <div className="pointer-events-none absolute top-2 left-2 right-2 aspect-square rounded-lg bg-black/40 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity flex items-center justify-center">
      <button type="button" onClick={handlePlay} disabled={!active || !isPlaybackAvailable || artworkBlocked || playing}
        aria-label={`${t("common.playAlbum")}: ${entry.album.title}`} title={t("common.playAlbum")}
        className="pointer-events-auto w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed focus-visible:outline-2 focus-visible:outline-white">
        <Play size={20} className="ml-1" fill="black" aria-hidden="true" />
      </button>
    </div>}
    <p className="mt-2 truncate text-sm font-semibold text-white" title={entry.album.title}>{entry.album.title || t("explore.untitled")}</p>
    <p className="truncate text-xs text-[#AAAAAA]" title={entry.album.artist}>{entry.album.artist || t("explore.artistNotProvided")}</p>
    <p className="mt-1 truncate text-[11px] text-[#777777]" title={origin}>{origin}</p>
    {playError && <p role="alert" className="text-xs text-red-300">{t("explore.albumPlayError")}</p>}
  </div>;
};

export const ExploreArtistArtwork: React.FC<{ group: ExploreArtistGroup; active: boolean; album?: ExploreEntry;
  currentAlbum?: () => boolean; onlineArtworkEnabled?: boolean; artworkBlocked?: boolean }> = ({ group, active, album, currentAlbum, onlineArtworkEnabled, artworkBlocked }) => {
  const initials = artistPortraitInitials(group.name);
  const initialsFallback = <span aria-hidden="true" className="flex h-full w-full items-center justify-center text-3xl font-medium text-[#AAAAAA]">
    {initials ?? <UserRound size={44} strokeWidth={1.25} />}
  </span>;
  const fallback = album && currentAlbum ? <ExploreAlbumCard artworkOnly fallback={initialsFallback} entry={album} revision={0} active={active}
    current={currentAlbum} onlineArtworkEnabled={onlineArtworkEnabled} artworkBlocked={artworkBlocked} onOpen={() => {}} />
    : <div className="h-full w-full rounded-full border border-[#333333] bg-[#202020]">{initialsFallback}</div>;
  return <div className="mx-auto h-32 w-32 shrink-0">{fallback}</div>;
};

export const ExploreAlbumShelf: React.FC<{ id: string; title: string; cards: React.ReactElement[]; active: boolean;
  waiting: boolean; resetRevision: number; positions: Record<string, number>; onRenew: () => void }> = ({ id, title, cards, active, waiting, resetRevision, positions, onRenew }) => {
  const { t } = useTranslation();
  const shelfNode = useRef<HTMLDivElement | null>(null);
  const shelfPosition = useRef(positions[id] ?? 0);
  const restoringShelf = useRef(true);
  const shelfActive = useRef(active); shelfActive.current = active && !waiting;
  const shelfReset = resetRevision;
  const lastReset = useRef(resetRevision);
  const shelfCount = waiting ? EXPLORE_SUGGESTION_COUNT : cards.length;
  const [shelfLayout, setShelfLayout] = useState(() => albumShelfLayout(0, shelfCount));
  const [shelfEdges, setShelfEdges] = useState({ previous: false, next: false });
  const updateShelfEdges = (node: HTMLDivElement) => {
    const next = albumShelfEdges(node.clientWidth, node.scrollWidth, node.scrollLeft);
    setShelfEdges(old => old.previous === next.previous && old.next === next.next ? old : next);
  };
  useLayoutEffect(() => {
    const node = shelfNode.current;
    if (!node || !active) return;
    const measure = () => {
      if (node.clientWidth <= 0) return; // Hidden details must not overwrite the saved position.
      const next = albumShelfLayout(node.clientWidth, shelfCount);
      setShelfLayout(old => old.viewportWidth === next.viewportWidth && old.cardWidth === next.cardWidth && old.visibleCount === next.visibleCount ? old : next);
      updateShelfEdges(node);
    };
    const scroll = () => {
      if (!shelfActive.current || node.clientWidth <= 0 || restoringShelf.current) return;
      shelfPosition.current = node.scrollLeft; positions[id] = node.scrollLeft; updateShelfEdges(node);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(node); node.addEventListener("scroll", scroll, { passive: true }); measure();
    return () => { restoringShelf.current = true; observer.disconnect(); node.removeEventListener("scroll", scroll); };
  }, [active, shelfCount]);
  useLayoutEffect(() => {
    const node = shelfNode.current;
    if (!node || !active || waiting || node.clientWidth <= 0 || shelfLayout.viewportWidth !== node.clientWidth) return;
    restoringShelf.current = true;
    if (lastReset.current !== resetRevision) { shelfPosition.current = 0; positions[id] = 0; lastReset.current = resetRevision; }
    const left = Math.max(0, Math.min(shelfPosition.current, node.scrollWidth - node.clientWidth));
    node.scrollTo({ left, behavior: "auto" }); shelfPosition.current = left; positions[id] = left;
    updateShelfEdges(node); restoringShelf.current = false;
  }, [active, waiting, shelfLayout, shelfCount, shelfReset]);
  const scrollShelf = (direction: -1 | 1 | "first" | "last") => {
    const node = shelfNode.current;
    if (!node || !active || waiting || node.clientWidth <= 0 || node.scrollWidth <= node.clientWidth + 1) return;
    const left = albumShelfTarget(node.clientWidth, node.scrollWidth, node.scrollLeft, shelfLayout, direction);
    node.scrollTo({ left, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  };
  return <section>
          <div className="flex items-center justify-between gap-4">
            <h2 className="text-lg font-bold text-white">{title}</h2>
            <div className="flex shrink-0 gap-2">
              <button type="button" disabled={!active || waiting || !cards.length} onClick={onRenew} aria-label={t("explore.renewSection", { title })} title={t("explore.renewSectionHint")} className="rounded-full p-2 text-[#AAAAAA] hover:bg-[#222222] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><RefreshCw size={16} /></button>
              <button type="button" aria-controls={id} aria-label={t("explore.previousSuggestions")} title={t("explore.previousSuggestions")}
                disabled={!active || waiting || !shelfEdges.previous} onClick={() => scrollShelf(-1)}
                className="rounded-full p-2 text-[#AAAAAA] hover:bg-[#222222] hover:text-white focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><ChevronLeft size={18} /></button>
              <button type="button" aria-controls={id} aria-label={t("explore.nextSuggestions")} title={t("explore.nextSuggestions")}
                disabled={!active || waiting || !shelfEdges.next} onClick={() => scrollShelf(1)}
                className="rounded-full p-2 text-[#AAAAAA] hover:bg-[#222222] hover:text-white focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><ChevronRight size={18} /></button>
            </div>
          </div>
          <div className="relative mt-3">
            <div ref={shelfNode} id={id} role="region" aria-label={title} aria-busy={waiting} tabIndex={0}
              onKeyDown={event => {
                if (event.target !== event.currentTarget || event.defaultPrevented || (!shelfEdges.previous && !shelfEdges.next) || !active || waiting) return;
                const directions = { ArrowLeft: -1, ArrowRight: 1, Home: "first", End: "last" } as const;
                const direction = directions[event.key as keyof typeof directions];
                if (direction !== undefined) { event.preventDefault(); scrollShelf(direction); }
              }}
              className="explore-album-shelf grid grid-flow-col gap-6 overflow-x-auto overscroll-x-contain snap-x snap-mandatory py-1 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
              style={{ gridAutoColumns: `${shelfLayout.cardWidth}px` }}>
              {waiting ? Array.from({ length: EXPLORE_SUGGESTION_COUNT }, (_, index) => <div key={index} aria-hidden="true" className="min-w-0 snap-start rounded-xl p-2">
                <div className="aspect-square rounded-lg bg-[#202020]" /><div className="mt-2 h-5 rounded bg-[#202020]" />
                <div className="mt-1 h-4 w-2/3 rounded bg-[#202020]" /><div className="mt-1 h-4 w-1/2 rounded bg-[#202020]" />
              </div>) : cards.map(card => <div key={card.key} className="min-w-0 snap-start">{card}</div>)}
            </div>
            {waiting && <p role="status" className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-[#AAAAAA]">{t("explore.creatingSuggestions")}</p>}
          </div>
          {!waiting && !cards.length && <p role="status" className="mt-4 text-xs text-[#999999]">{t("explore.noSuggestions")}</p>}
        </section>;
};

interface Props {
  context: ExploreContext;
  active: boolean;
  sourceSelector?: React.ReactNode;
  ensureLocal: () => void;
  isCurrent: (entry: ExploreEntry) => boolean;
  onOpen: (entry: ExploreEntry) => void;
  onPlay?: (entry: ExploreEntry, current: () => boolean) => Promise<void>;
  isPlaybackAvailable?: boolean;
  onlineArtworkEnabled?: boolean;
  artworkBlocked?: boolean;
  configurationReady?: boolean;
  onConfigure?: () => void;
  retryLocal?: () => void;
  canRetryLocal?: boolean;
  loadingActivity?: "indexing" | "organizing" | null;
}
export const ExploreView: React.FC<Props> = ({ context, active, ensureLocal, isCurrent, onOpen, onPlay, isPlaybackAvailable = false, sourceSelector, onlineArtworkEnabled = false, artworkBlocked = false, configurationReady = true, onConfigure, retryLocal, canRetryLocal = true, loadingActivity = null }) => {
  const { t } = useTranslation();
  const session = useMemo(() => createExploreSession(), []);
  session.syncContext(configurationReady ? context : { ...context, local: { ...context.local, state: "loading" } }, active, false);
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const ensure = useRef(ensureLocal); ensure.current = ensureLocal;
  useEffect(() => { session.activate(); return () => session.close(); }, [session]);
  useEffect(() => { if (active && configurationReady && context.local.state !== "not_configured") ensure.current(); }, [active, configurationReady, context.local.state]);
  const waiting = state.preparing || context.local.state === "loading";
  const slice = waiting ? null : state.slice;
  const continuousGenre = slice?.group.kind === "genre";
  const selected = continuousGenre ? [] : slice ? session.sliceEntries() : session.visibleEntries();
  const artists = session.visibleArtists(), genres = session.visibleGenres();
  const genreColors = assignExploreGenreColors(genres.map(group => group.value));
  const assignedGenreIcons = assignExploreGenreIcons(genres.map(group => group.value));
  const shelfPositions = useRef<Record<string, number>>({});
  const genrePositions = useRef<Record<string, number>>({});
  const sliceIdentity = useRef<string | null>(null);
  const viewScope = useRef(context.local.sourceId ?? null);
  const scrollNode = useRef<HTMLDivElement | null>(null);
  const mainPosition = useRef(0);
  if (viewScope.current !== (context.local.sourceId ?? null)) {
    viewScope.current = context.local.sourceId ?? null;
    shelfPositions.current = {}; genrePositions.current = {}; sliceIdentity.current = null; mainPosition.current = 0;
  }
  useLayoutEffect(() => { if (active && !waiting && scrollNode.current) scrollNode.current.scrollTop = mainPosition.current; }, [active, waiting, context.local.sourceId]);

  if (!state.slice || state.slice.group.kind !== "genre") sliceIdentity.current = null;
  else if (sliceIdentity.current !== state.slice.group.key) { genrePositions.current[state.slice.group.key] = 0; sliceIdentity.current = state.slice.group.key; }
  const renew = () => {
    const before = session.getSnapshot(); session.renew();
    if (session.getSnapshot() !== before) Object.keys(shelfPositions.current).forEach(id => { shelfPositions.current[id] = 0; });
  };
  const renewShelf = (id: ExploreShelfId) => {
    const before = session.getSnapshot(); session.renewShelf(id);
    if (session.getSnapshot() !== before) shelfPositions.current[`explore-${id === "albums" ? "album" : id}-shelf`] = 0;
  };
  // Online enrichment is confined to suggested albums; slices may reuse the same cache.
  const albumCards = selected.map(entry => <ExploreAlbumCard key={entry.key} entry={entry}
    revision={state.revision} active={active} onPlay={onPlay} isPlaybackAvailable={isPlaybackAvailable} onlineArtworkEnabled={!slice && onlineArtworkEnabled} artworkBlocked={artworkBlocked} current={() => (slice ? session.canUseSlice(entry, state.revision) : session.canUse(entry, state.revision)) && isCurrent(entry)}
    onOpen={() => { if ((slice ? session.canUseSlice(entry, state.revision) : session.canUse(entry, state.revision)) && isCurrent(entry)) onOpen(entry); }} />);
  const albumGrid = <div className="mt-3 grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">{albumCards}</div>;
  const configured = context.local.state !== "not_configured";
  const scope = t("explore.localScope");
  const configureButton = onConfigure && <button id="explore-configure-collection" type="button" disabled={!active} onClick={onConfigure}
    className="mt-6 rounded-lg bg-[#E5A00D] px-5 py-3 text-sm font-semibold text-black hover:bg-[#F5B01D] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white disabled:opacity-40">{t("explore.configureCollection")}</button>;
  if (!configurationReady) return <main className="flex min-h-0 flex-1 items-center justify-center bg-[#121212] p-8"><p role="status" className="text-sm text-[#AAAAAA]">{t("explore.creatingSuggestions")}</p></main>;
  if (!configured) return <main className="flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-y-auto bg-[#121212] p-8">
    <div className="max-w-lg text-center">
      <div aria-hidden="true" className="mx-auto mb-6 flex h-20 w-20 items-center justify-center rounded-full border border-[#E5A00D]/20 bg-[#E5A00D]/10 text-[#E5A00D]"><Compass size={36} strokeWidth={1.5} /></div>
      <h1 className="text-2xl font-bold leading-snug text-white">{t("explore.collectionStartTitle")}</h1>
      <p className="mt-4 text-sm leading-relaxed text-[#AAAAAA]">{t("explore.collectionStartText")}</p>
      {configureButton}
    </div>
  </main>;
  const unavailable = context.local.state === "unavailable";
  return <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[#121212] select-none">
    <header className="flex flex-wrap items-center justify-between gap-4 border-b border-[#222222] px-8 pt-8 pb-4">
      <div className="min-w-0">
        <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight text-white"><Compass size={22} className="shrink-0 text-[#E5A00D]" />{t("explore.title")}</h1>
        <p className="mt-1 text-xs text-[#888888]">{scope}</p>
      </div>
      {sourceSelector}
      {!slice && (waiting || selected.length > 0) && <button type="button" disabled={!active || waiting || session.candidateCount() < 2} onClick={renew}
        title={t("explore.renewHint")} className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">
        <RefreshCw size={14} />{t("explore.renew")}
      </button>}
    </header>
    <div ref={scrollNode} onScroll={event => { if (active && !waiting) mainPosition.current = event.currentTarget.scrollTop; }} className={`min-h-0 flex-1 px-8 py-5 ${continuousGenre ? "flex flex-col overflow-hidden" : "overflow-y-auto"}`}>
      {context.local.state !== "ready" && context.local.state !== "not_configured" && <p className="text-xs text-[#999999]"
        role={unavailable ? "alert" : "status"}>{t("explore.sources.local")}: {context.local.state === "loading" && loadingActivity ? t(loadingActivity === "indexing" ? "settings.updatingLibrary" : "localBrowser.organizing") : t(`explore.states.${context.local.state}`)}</p>}
      {context.local.state === "unavailable" && retryLocal && <button type="button" disabled={!active || !canRetryLocal} onClick={retryLocal}
        className="mt-2 rounded px-2 py-1 text-xs text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">{t("explore.retryLocal")}</button>}
      {state.sliceInvalidated && <p role="status" className="mt-3 text-xs text-[#999999]">{t("explore.sliceInvalidated")}</p>}
      {!slice && !waiting && !selected.length ? <section className="flex min-h-64 flex-col items-center justify-center py-12 text-center">
        <p role={unavailable ? "alert" : "status"} className="max-w-md text-sm text-[#AAAAAA]">{t(unavailable ? "explore.collectionUnavailable" : "explore.collectionEmpty")}</p>
        {configureButton}
      </section> : slice ? <section className={continuousGenre ? "mt-4 flex min-h-0 flex-1 flex-col" : "mt-4"}>
        <button type="button" disabled={!active} onClick={() => session.leaveSlice(state.revision)} className="mb-4 flex items-center gap-2 rounded px-1 py-1 text-xs text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D]">
          <ArrowLeft size={16} />{t("explore.backToShelves")}
        </button>
        <h2 className="text-lg font-bold text-white">{slice.group.name}</h2>
        <p className="mt-1 text-xs text-[#888888]">{slice.group.kind === "artist" ? t(`explore.sources.${slice.group.source}`) : t("explore.genreSlice")}
          {" · "}{t("explore.sliceCount", { count: slice.entries.length })}</p>
        {continuousGenre ? <ExploreGenreSlice id={slice.group.key} label={slice.group.name} entries={slice.entries}
          active={active} positions={genrePositions.current} renderItem={entry => <ExploreAlbumCard key={entry.key} entry={entry}
            revision={state.revision} active={active} onPlay={onPlay} isPlaybackAvailable={isPlaybackAvailable} onlineArtworkEnabled={false} artworkBlocked={artworkBlocked}
            current={() => session.canUseSlice(entry, state.revision) && isCurrent(entry)}
            onOpen={() => { if (session.canUseSlice(entry, state.revision) && isCurrent(entry)) onOpen(entry); }} />} /> : albumGrid}
        {!slice.entries.length && <p role="status" className="mt-4 text-xs text-[#999999]">{t("explore.noSliceAlbums")}</p>}
        {!continuousGenre && slice.entries.length > EXPLORE_PAGE_SIZE && <nav aria-label={t("explore.pagination")} className="mt-5 flex items-center gap-4 text-xs text-[#AAAAAA]">
          <button type="button" disabled={!active || slice.page === 0} onClick={() => session.setPage(slice.page - 1, state.revision)} className="rounded px-2 py-1 hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">{t("explore.previousPage")}</button>
          <span>{t("explore.page", { current: slice.page + 1, total: Math.ceil(slice.entries.length / EXPLORE_PAGE_SIZE) })}</span>
          <button type="button" disabled={!active || (slice.page + 1) * EXPLORE_PAGE_SIZE >= slice.entries.length} onClick={() => session.setPage(slice.page + 1, state.revision)} className="rounded px-2 py-1 hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">{t("explore.nextPage")}</button>
        </nav>}
      </section> : <div className="space-y-8 pt-4">{[
        <ExploreAlbumShelf key="albums" id="explore-album-shelf" title={t("explore.shelf")} cards={albumCards}
          active={active && !slice} waiting={waiting} positions={shelfPositions.current} resetRevision={state.resets.albums ?? 0} onRenew={() => renewShelf("albums")} />,
        state.themes.map(theme => <ExploreAlbumShelf key={theme.id} id={`explore-${theme.id}-shelf`}
          title={theme.id === "genre" ? t("explore.genreTheme", { genre: theme.label }) : t(theme.current ? "explore.currentDecade" : "explore.pastDecade", { decade: theme.value })}
          cards={theme.selected.map(entry => <ExploreAlbumCard key={entry.key} entry={entry} revision={state.revision} active={active} onPlay={onPlay} isPlaybackAvailable={isPlaybackAvailable}
            onlineArtworkEnabled={onlineArtworkEnabled} artworkBlocked={artworkBlocked}
            current={() => session.canUseShelf(entry, state.revision, theme.id) && isCurrent(entry)}
            onOpen={() => { if (session.canUseShelf(entry, state.revision, theme.id) && isCurrent(entry)) onOpen(entry); }} />)}
          active={active && !slice} waiting={waiting} positions={shelfPositions.current} resetRevision={state.resets[theme.id] ?? 0} onRenew={() => renewShelf(theme.id)} />),
        <section key="artists">
          <div className="flex items-center justify-between gap-4"><h2 className="text-lg font-bold text-white">{t("explore.artists")}</h2><button type="button" disabled={!active || waiting || !artists.length} onClick={() => renewShelf("artists")} aria-label={t("explore.renewSection", { title: t("explore.artists") })} className="rounded-full p-2 text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><RefreshCw size={16} /></button></div>
          <div className="mt-3 grid grid-cols-[repeat(auto-fit,minmax(170px,1fr))] gap-6">{waiting
            ? Array.from({ length: 4 }, (_, index) => <div key={index} aria-hidden="true" className="rounded-xl bg-[#1A1A1A] px-4 py-5">
              <div className="mx-auto h-32 w-32 rounded-full bg-[#202020]" /><div className="mt-4 h-5 rounded bg-[#202020]" /><div className="mt-1 h-4 rounded bg-[#202020]" />
            </div>) : artists.map(group => {
              const origin = `${t(`explore.sources.${group.source}`)}${group.albumCount !== undefined ? ` · ${t("explore.sliceCount", { count: group.albumCount })}` : ""}`;
              const artistAlbum = session.artistAlbum(group, entry => !!localArtworkResolver.get(entry.album.id));
              return <button key={group.key} type="button" disabled={!active} aria-label={`${group.name} · ${origin}`}
                onClick={() => session.openSlice(group, state.revision)}
                className="min-w-0 rounded-xl bg-[#1A1A1A] px-4 py-5 text-center hover:bg-[#222222] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">
                <ExploreArtistArtwork album={artistAlbum}
                  currentAlbum={() => !!artistAlbum && session.validDetail(artistAlbum) && isCurrent(artistAlbum)}
                  onlineArtworkEnabled={onlineArtworkEnabled} artworkBlocked={artworkBlocked} group={group} active={active} />
                <p className="mt-4 truncate text-sm font-semibold text-white" title={group.name}>{group.name}</p>
                <p className="mt-1 truncate text-[11px] text-[#888888]" title={origin}>{origin}</p>
              </button>;
            })}</div>
          {!waiting && !artists.length && <p className="mt-3 text-xs text-[#888888]">{t("explore.noArtists")}</p>}
        </section>,
        <section key="genres">
          <div className="flex items-center justify-between gap-4"><h2 className="text-lg font-bold text-white">{t("explore.genres")}</h2><button type="button" disabled={!active || waiting || !genres.length} onClick={() => renewShelf("genres")} aria-label={t("explore.renewSection", { title: t("explore.genres") })} className="rounded-full p-2 text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><RefreshCw size={16} /></button></div>
          <div className="mt-3 grid grid-cols-[repeat(auto-fit,minmax(min(190px,100%),1fr))] gap-4">{waiting
            ? Array.from({ length: 4 }, (_, index) => <div key={index} aria-hidden="true" className="h-40 rounded-xl bg-[#1A1A1A]" />)
            : genres.map(group => {
              const style = exploreGenreCardStyle(group.value), Icon = genreIcons[assignedGenreIcons[group.value] ?? style.icon];
              const count = t("explore.sliceCount", { count: session.genreAlbumCount(group) });
              const origin = group.sources.filter(source => context[source].state === "ready").map(source => t(`explore.sources.${source}`)).join(" / ");
              return <button key={group.key} type="button" disabled={!active} aria-label={`${group.name} · ${count} · ${origin}`}
                onClick={() => session.openSlice(group, state.revision)}
                style={{ backgroundColor: genreColors[group.value] ?? style.background, backgroundImage: "linear-gradient(135deg, rgba(255,255,255,0.06), transparent 70%)" }}
                className="relative flex h-40 min-w-0 flex-col justify-between overflow-hidden rounded-xl p-5 text-left text-white transition-[filter] hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] focus-visible:ring-offset-2 focus-visible:ring-offset-[#121212] disabled:opacity-60">
                <Icon aria-hidden="true" size={56} strokeWidth={1.4} className="pointer-events-none absolute right-4 bottom-4 opacity-20" />
                <p className="relative line-clamp-2 break-words text-xl font-bold leading-tight [overflow-wrap:anywhere]" title={group.name}>{group.name}</p>
                <div className="relative min-w-0 pr-16 text-[#F3EDE7]">
                  <p className="text-xs font-medium">{count}</p>
                  <p className="mt-1 truncate text-[11px]" title={origin}>{origin}</p>
                </div>
              </button>;
            })}</div>
          {!waiting && !genres.length && <p className="mt-3 text-xs text-[#888888]">{t("explore.noGenres")}</p>}
        </section>].flat().sort((a, b) => state.sectionOrder.indexOf(String(a.key) as ExploreShelfId) - state.sectionOrder.indexOf(String(b.key) as ExploreShelfId))}
      </div>}
    </div>
  </main>;
};
