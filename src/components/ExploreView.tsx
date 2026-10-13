import React, { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { ArrowLeft, Compass, RefreshCw, UserRound, ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { createExploreSession, exploreOriginName, EXPLORE_PAGE_SIZE, EXPLORE_SUGGESTION_COUNT } from "../utils/explore";
import { albumShelfEdges, albumShelfLayout, albumShelfTarget } from "../utils/exploreAlbumShelf";
import type { ExploreArtistGroup, ExploreContext, ExploreEntry } from "../utils/explore";
import { artistPortraitInitials } from "../utils/plexArtistPortraits";
import type { ArtistPortraitResult, PlexArtistPortraitCache } from "../utils/plexArtistPortraits";
import { localArtworkResolver } from "../utils/localArtworkSession";
import { PlexImage } from "./PlexImage";
import { AlbumArtworkPlaceholder } from "./AlbumArtworkPlaceholder";

export const ExploreAlbumCard: React.FC<{ entry: ExploreEntry; current: () => boolean; revision: number; active: boolean; onOpen: () => void }> = ({ entry, current, active, onOpen }) => {
  const { t } = useTranslation();
  const [cover, setCover] = useState<string | null>(null);
  const live = useRef(current); live.current = current;
  const origin = `${t(`explore.sources.${entry.source}`)} · ${exploreOriginName(entry) ?? t("explore.unknownOrigin")}`;
  useEffect(() => {
    if (entry.source !== "local" || !active || !live.current()) return;
    let mounted = true;
    const valid = () => live.current();
    const cached = localArtworkResolver.get(entry.album.id);
    setCover(cached);
    if (cached) return;
    void localArtworkResolver.resolveLocalOnly(entry.album, () => !mounted || !valid()).then(value => {
      if (mounted && valid()) setCover(value);
    }).catch(() => { /* Artwork failure leaves a placeholder, not a catalog failure. */ });
    return () => { mounted = false; };
  }, [entry.album, entry.source, active]);
  return <button type="button" onClick={() => { if (live.current()) onOpen(); }} disabled={!active}
    className="w-full min-w-0 rounded-xl p-2 text-left hover:bg-[#222222] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-60">
    <div className="aspect-square overflow-hidden rounded-lg bg-[#202020]">
      {entry.source === "plex" && entry.album.thumb && active ? <PlexImage image={entry.album.thumb} alt="" className="h-full w-full object-cover" fallback={<AlbumArtworkPlaceholder />} />
        : cover ? <img src={cover} alt="" className="h-full w-full object-cover" /> : <AlbumArtworkPlaceholder />}
    </div>
    <p className="mt-2 truncate text-sm font-semibold text-white" title={entry.album.title}>{entry.album.title || t("explore.untitled")}</p>
    <p className="truncate text-xs text-[#AAAAAA]" title={entry.album.artist}>{entry.album.artist || t("explore.artistNotProvided")}</p>
    <p className="mt-1 truncate text-[11px] text-[#777777]" title={origin}>{origin}</p>
  </button>;
};

export const ExploreArtistPortrait: React.FC<{ group: ExploreArtistGroup; identity: unknown; serverId: string;
  active: boolean; cache?: PlexArtistPortraitCache }> = ({ group, identity, serverId, active, cache }) => {
  const [resolved, setResolved] = useState<{ identity: unknown; serverId: string; ratingKey: string; result: ArtistPortraitResult } | null>(null);
  const ratingKey = group.source === "plex" ? group.ratingKey : undefined;
  useEffect(() => {
    if (!active || !cache || !ratingKey) return;
    let current = true;
    const lease = cache.acquire(identity, serverId, ratingKey);
    void lease.promise.then(result => { if (current) setResolved({ identity, serverId, ratingKey, result }); });
    return () => { current = false; lease.release(); };
  }, [cache, identity, serverId, ratingKey, active]);
  const result = active && ratingKey && cache?.isCurrent(identity, serverId)
    ? cache.peek(identity, serverId, ratingKey) ?? (resolved && resolved.identity === identity && resolved.serverId === serverId
      && resolved.ratingKey === ratingKey ? resolved.result : null) : null;
  const initials = artistPortraitInitials(group.name);
  const fallback = <span aria-hidden="true" className="flex h-full w-full items-center justify-center text-3xl font-medium text-[#AAAAAA]">
    {initials ?? <UserRound size={44} strokeWidth={1.25} />}
  </span>;
  return <div className="mx-auto h-32 w-32 shrink-0 overflow-hidden rounded-full border border-[#333333] bg-[#202020]">
    {result?.status === "ready" ? <PlexImage image={result.image} cacheScope={typeof identity === "object" && identity !== null ? identity : undefined}
      alt="" className="h-full w-full object-cover" fallback={fallback} /> : fallback}
  </div>;
};

interface Props {
  context: ExploreContext;
  active: boolean;
  libraryTitle: string | null;
  ensureLocal: () => void;
  isCurrent: (entry: ExploreEntry) => boolean;
  onOpen: (entry: ExploreEntry) => void;
  plexProgress?: { loading: boolean; failedLibraries: readonly string[]; discoveryFailed: boolean; complete: boolean };
  retryPlex?: () => void;
  portraits?: PlexArtistPortraitCache;
}
export const ExploreView: React.FC<Props> = ({ context, active, libraryTitle, ensureLocal, isCurrent, onOpen, plexProgress, retryPlex, portraits }) => {
  const { t } = useTranslation();
  const session = useMemo(() => createExploreSession(), []);
  session.syncContext(context, active, false);
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const ensure = useRef(ensureLocal); ensure.current = ensureLocal;
  useEffect(() => { session.activate(); return () => session.close(); }, [session]);
  useEffect(() => { if (active) ensure.current(); }, [active, context.local.state]);
  const slice = state.slice;
  const selected = slice ? session.sliceEntries() : session.visibleEntries();
  const artists = session.visibleArtists(), genres = session.visibleGenres();
  const shelfNode = useRef<HTMLDivElement | null>(null);
  const shelfPosition = useRef(0);
  const restoringShelf = useRef(true);
  const shelfActive = useRef(active); shelfActive.current = active;
  const [shelfReset, setShelfReset] = useState(0);
  const shelfCount = state.preparing ? EXPLORE_SUGGESTION_COUNT : session.visibleEntries().length;
  const [shelfLayout, setShelfLayout] = useState(() => albumShelfLayout(0, shelfCount));
  const [shelfEdges, setShelfEdges] = useState({ previous: false, next: false });
  const updateShelfEdges = (node: HTMLDivElement) => {
    const next = albumShelfEdges(node.clientWidth, node.scrollWidth, node.scrollLeft);
    setShelfEdges(old => old.previous === next.previous && old.next === next.next ? old : next);
  };
  useLayoutEffect(() => {
    const node = shelfNode.current;
    if (!node || !active || slice) return;
    const measure = () => {
      if (node.clientWidth <= 0) return; // Hidden details must not overwrite the saved position.
      const next = albumShelfLayout(node.clientWidth, shelfCount);
      setShelfLayout(old => old.viewportWidth === next.viewportWidth && old.cardWidth === next.cardWidth && old.visibleCount === next.visibleCount ? old : next);
      updateShelfEdges(node);
    };
    const scroll = () => {
      if (!shelfActive.current || node.clientWidth <= 0 || restoringShelf.current) return;
      shelfPosition.current = node.scrollLeft; updateShelfEdges(node);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(node); node.addEventListener("scroll", scroll, { passive: true }); measure();
    return () => { restoringShelf.current = true; observer.disconnect(); node.removeEventListener("scroll", scroll); };
  }, [active, !!slice, shelfCount]);
  useLayoutEffect(() => {
    const node = shelfNode.current;
    if (!node || !active || slice || node.clientWidth <= 0 || shelfLayout.viewportWidth !== node.clientWidth) return;
    restoringShelf.current = true;
    const left = Math.max(0, Math.min(shelfPosition.current, node.scrollWidth - node.clientWidth));
    node.scrollTo({ left, behavior: "auto" }); shelfPosition.current = left;
    updateShelfEdges(node); restoringShelf.current = false;
  }, [active, !!slice, shelfLayout, shelfCount, shelfReset]);
  const scrollShelf = (direction: -1 | 1 | "first" | "last") => {
    const node = shelfNode.current;
    if (!node || !active || state.preparing || node.clientWidth <= 0 || node.scrollWidth <= node.clientWidth + 1) return;
    const left = albumShelfTarget(node.clientWidth, node.scrollWidth, node.scrollLeft, shelfLayout, direction);
    node.scrollTo({ left, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  };
  const renew = () => {
    const before = session.getSnapshot(); session.renew();
    if (session.getSnapshot() !== before) { shelfPosition.current = 0; setShelfReset(old => old + 1); }
  };
  const albumCards = selected.map(entry => <ExploreAlbumCard key={entry.key} entry={entry}
    revision={state.revision} active={active} current={() => (slice ? session.canUseSlice(entry, state.revision) : session.canUse(entry, state.revision)) && isCurrent(entry)}
    onOpen={() => { if ((slice ? session.canUseSlice(entry, state.revision) : session.canUse(entry, state.revision)) && isCurrent(entry)) onOpen(entry); }} />);
  const albumGrid = <div className="mt-3 grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">{albumCards}</div>;
  const groupButtonClass = "min-w-0 rounded-lg bg-[#1A1A1A] px-4 py-3 text-left hover:bg-[#222222] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40";
  return <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[#121212] select-none">
    <header className="flex flex-wrap items-center justify-between gap-4 border-b border-[#222222] px-8 pt-8 pb-4">
      <div className="min-w-0">
        <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight text-white"><Compass size={22} className="shrink-0 text-[#E5A00D]" />{t("explore.title")}</h1>
        <p className="mt-1 text-xs text-[#888888]">{plexProgress ? t("explore.aggregateScope") : t("explore.scope", { library: libraryTitle ?? t("explore.noLibrary") })}</p>
      </div>
      {!slice && <button type="button" disabled={!active || state.preparing || session.candidateCount() < 2} onClick={renew}
        title={t("explore.renewHint")} className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">
        <RefreshCw size={14} />{t("explore.renew")}
      </button>}
    </header>
    <div className="min-h-0 flex-1 overflow-y-auto px-8 py-5">
      <div className="flex flex-wrap gap-x-5 gap-y-1 text-xs text-[#999999]">{(["local", "plex"] as const).filter(source => context[source].state !== "ready").map(source => <p key={source}
        role={context[source].state === "unavailable" ? "alert" : "status"}>
        {t(`explore.sources.${source}`)}: {t(`explore.states.${context[source].state}`)}
      </p>)}</div>
      {plexProgress && !plexProgress.complete && <div className="mt-2 flex flex-wrap items-center gap-3 text-xs text-[#999999]">
        {plexProgress.loading && <p role="status">{t("explore.librariesLoading")}</p>}
        {plexProgress.failedLibraries.length > 0 && <p role="status">{t("explore.librariesFailed", { libraries: plexProgress.failedLibraries.join(", ") })}</p>}
        {plexProgress.discoveryFailed && <p role="status">{t("explore.librariesUnavailable")}</p>}
        {(plexProgress.discoveryFailed || plexProgress.failedLibraries.length > 0) && <button type="button" disabled={!active} onClick={retryPlex}
          className="rounded px-2 py-1 text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">{t("explore.retryLibraries")}</button>}
      </div>}
      {state.sliceInvalidated && <p role="status" className="mt-3 text-xs text-[#999999]">{t("explore.sliceInvalidated")}</p>}
      {slice ? <section className="mt-4">
        <button type="button" disabled={!active} onClick={() => session.leaveSlice(state.revision)} className="mb-4 flex items-center gap-2 rounded px-1 py-1 text-xs text-[#AAAAAA] hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D]">
          <ArrowLeft size={16} />{t("explore.backToShelves")}
        </button>
        <h2 className="text-lg font-bold text-white">{slice.group.name}</h2>
        <p className="mt-1 text-xs text-[#888888]">{slice.group.kind === "artist" ? t(`explore.sources.${slice.group.source}`) : t("explore.genreSlice")}
          {" · "}{t("explore.sliceCount", { count: slice.entries.length })}</p>
        {albumGrid}
        {!selected.length && <p role="status" className="mt-4 text-xs text-[#999999]">{t("explore.noSliceAlbums")}</p>}
        {slice.entries.length > EXPLORE_PAGE_SIZE && <nav aria-label={t("explore.pagination")} className="mt-5 flex items-center gap-4 text-xs text-[#AAAAAA]">
          <button type="button" disabled={!active || slice.page === 0} onClick={() => session.setPage(slice.page - 1, state.revision)} className="rounded px-2 py-1 hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">{t("explore.previousPage")}</button>
          <span>{t("explore.page", { current: slice.page + 1, total: Math.ceil(slice.entries.length / EXPLORE_PAGE_SIZE) })}</span>
          <button type="button" disabled={!active || (slice.page + 1) * EXPLORE_PAGE_SIZE >= slice.entries.length} onClick={() => session.setPage(slice.page + 1, state.revision)} className="rounded px-2 py-1 hover:text-white focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">{t("explore.nextPage")}</button>
        </nav>}
      </section> : <div className="space-y-8 pt-4">
        <section>
          <div className="flex items-center justify-between gap-4">
            <h2 className="text-lg font-bold text-white">{t("explore.shelf")}</h2>
            <div className="flex shrink-0 gap-2">
              <button type="button" aria-controls="explore-album-shelf" aria-label={t("explore.previousSuggestions")} title={t("explore.previousSuggestions")}
                disabled={!active || state.preparing || !shelfEdges.previous} onClick={() => scrollShelf(-1)}
                className="rounded-full p-2 text-[#AAAAAA] hover:bg-[#222222] hover:text-white focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><ChevronLeft size={18} /></button>
              <button type="button" aria-controls="explore-album-shelf" aria-label={t("explore.nextSuggestions")} title={t("explore.nextSuggestions")}
                disabled={!active || state.preparing || !shelfEdges.next} onClick={() => scrollShelf(1)}
                className="rounded-full p-2 text-[#AAAAAA] hover:bg-[#222222] hover:text-white focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-30"><ChevronRight size={18} /></button>
            </div>
          </div>
          <div className="relative mt-3">
            <div ref={shelfNode} id="explore-album-shelf" role="region" aria-label={t("explore.shelf")} aria-busy={state.preparing} tabIndex={0}
              onKeyDown={event => {
                if (event.target !== event.currentTarget || event.defaultPrevented || (!shelfEdges.previous && !shelfEdges.next) || !active || state.preparing) return;
                const directions = { ArrowLeft: -1, ArrowRight: 1, Home: "first", End: "last" } as const;
                const direction = directions[event.key as keyof typeof directions];
                if (direction !== undefined) { event.preventDefault(); scrollShelf(direction); }
              }}
              className="grid grid-flow-col gap-6 overflow-x-auto overscroll-x-contain snap-x snap-mandatory py-1 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
              style={{ gridAutoColumns: `${shelfLayout.cardWidth}px` }}>
              {state.preparing ? Array.from({ length: EXPLORE_SUGGESTION_COUNT }, (_, index) => <div key={index} aria-hidden="true" className="min-w-0 snap-start rounded-xl p-2">
                <div className="aspect-square rounded-lg bg-[#202020]" /><div className="mt-2 h-5 rounded bg-[#202020]" />
                <div className="mt-1 h-4 w-2/3 rounded bg-[#202020]" /><div className="mt-1 h-4 w-1/2 rounded bg-[#202020]" />
              </div>) : albumCards.map(card => <div key={card.key} className="min-w-0 snap-start">{card}</div>)}
            </div>
            {state.preparing && <p role="status" className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-[#AAAAAA]">{t("explore.creatingSuggestions")}</p>}
          </div>
          {!state.preparing && !selected.length && <p role="status" className="mt-4 text-xs text-[#999999]">{t("explore.noSuggestions")}</p>}
        </section>
        <section>
          <h2 className="text-lg font-bold text-white">{t("explore.artists")}</h2>
          <div className="mt-3 grid grid-cols-[repeat(auto-fit,minmax(170px,1fr))] gap-6">{state.preparing
            ? Array.from({ length: 4 }, (_, index) => <div key={index} aria-hidden="true" className="rounded-xl bg-[#1A1A1A] px-4 py-5">
              <div className="mx-auto h-32 w-32 rounded-full bg-[#202020]" /><div className="mt-4 h-5 rounded bg-[#202020]" /><div className="mt-1 h-4 rounded bg-[#202020]" />
            </div>) : artists.map(group => {
              const origin = `${t(`explore.sources.${group.source}`)}${group.libraryTitle ? ` · ${group.libraryTitle}` : ""}${group.albumCount !== undefined ? ` · ${t("explore.sliceCount", { count: group.albumCount })}` : ""}`;
              return <button key={group.key} type="button" disabled={!active} aria-label={`${group.name} · ${origin}`}
                onClick={() => session.openSlice(group, state.revision)}
                className="min-w-0 rounded-xl bg-[#1A1A1A] px-4 py-5 text-center hover:bg-[#222222] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:opacity-40">
                <ExploreArtistPortrait group={group} identity={context.plex.identity} serverId={context.plex.serverId} active={active} cache={portraits} />
                <p className="mt-4 truncate text-sm font-semibold text-white" title={group.name}>{group.name}</p>
                <p className="mt-1 truncate text-[11px] text-[#888888]" title={origin}>{origin}</p>
              </button>;
            })}</div>
          {!state.preparing && !artists.length && <p className="mt-3 text-xs text-[#888888]">{t("explore.noArtists")}</p>}
        </section>
        <section>
          <h2 className="text-lg font-bold text-white">{t("explore.genres")}</h2>
          <div className="mt-3 grid max-w-3xl grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">{state.preparing ? Array.from({ length: 4 }, (_, index) => <div key={index} aria-hidden="true" className="h-16 rounded-lg bg-[#1A1A1A]" />) : genres.map(group => <button key={group.key} type="button" disabled={!active}
            onClick={() => session.openSlice(group, state.revision)} className={groupButtonClass}>
            <p className="truncate text-sm font-semibold text-white" title={group.name}>{group.name}</p>
            <p className="mt-1 text-[11px] text-[#888888]">{group.sources.map(source => t(`explore.sources.${source}`)).join(" / ")}</p>
          </button>)}</div>
          {!state.preparing && !genres.length && <p className="mt-3 text-xs text-[#888888]">{t("explore.noGenres")}</p>}
        </section>
      </div>}
    </div>
  </main>;
};
