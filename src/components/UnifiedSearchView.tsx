import { useTrackSelection } from "./TrackSelection";
import { useQueueInsertion } from "./QueueInsertionMenu";
import { plexQueueTrack } from "../utils/queueActions";
import { loadQueuedAlbum } from "../utils/queueAlbumLoader";
import React, { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Search, Disc3, Users, Music2, ArrowLeft, ArrowUpRight, Play, LoaderCircle, RefreshCw, X, Folder, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppConfig } from "../types/config";
import type { PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";
import { plexService } from "../services/plex";
import { localAlbumCatalog } from "../utils/localAlbumCatalogSession";
import { confirmedCatalogPlaybackAllowed } from "../utils/localAlbumCatalog";
import { SearchLane, localSearchIndex, localTrackRows, plexSearchPreview, rowText, searchQuery, searchTrackMetadata,
  sortSearchRows, SEARCH_PREVIEW_LIMIT, type SearchKind, type SearchPreview, type SearchRow, type SearchSource } from "../utils/unifiedSearch";
import { logError } from "../utils/frontendDiagnostics";
import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { PlaybackRequestNotice } from "./PlaybackRequestNotice";
import { LocalBrowserView } from "./LocalBrowserView";
import { AlbumView } from "./AlbumView";
import { ArtistView } from "./ArtistView";

interface Props {
  config: AppConfig;
  plexIdentity: object;
  status: PlaybackStatus;
  isPlaybackAvailable: boolean;
  catalogRefreshRevision: number;
  active: boolean;
  onFavoritesChanged: () => void;
}
type DetailRow = Exclude<SearchRow, { kind: "track" }>;

export const UnifiedSearchView: React.FC<Props> = ({ config, plexIdentity, status, isPlaybackAvailable, catalogRefreshRevision, active, onFavoritesChanged }) => {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const [source, setSource] = useState<SearchSource>("all");
  const [kind, setKind] = useState<SearchKind>("all");
  const [retry, setRetry] = useState(0);
  const [detail, setDetail] = useState<{ row: DetailRow; identity: unknown } | null>(null);
  const catalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const localConfigured = config.local_folders.length > 0;
  const plexConfigured = Boolean(config.plex_token.trim());
  const scope = source === "local" && !localConfigured || source === "plex" && !plexConfigured ? "all" : source;
  const query = searchQuery(text);
  const localReady = isPlaybackAvailable && confirmedCatalogPlaybackAllowed(catalog, status.is_updating);
  const roots = JSON.stringify(config.local_folders);
  const localIdentity = useMemo(() => ({}), [catalog.albums, roots]);
  // Search responses follow catalog generations; an open destination follows its
  // configured sources. Routine refreshes must not unmount that destination.
  const localDetailIdentity = useMemo(() => ({}), [roots]);
  const localIndex = useMemo(() => localSearchIndex(catalog.albums), [catalog.albums]);
  const localMetadata = useMemo(() => query && localConfigured && scope !== "plex" && localReady ? localIndex(query) : { rows: [], limited: false },
    [query, localConfigured, scope, localReady, localIndex]);
  const localMetadataRef = useRef(localMetadata); localMetadataRef.current = localMetadata;
  const localLane = useMemo(() => new SearchLane<SearchPreview>(error => logError("search.local", error)), []);
  const plexLane = useMemo(() => new SearchLane<SearchPreview>(error => logError("search.plex", error)), []);
  const local = useSyncExternalStore(localLane.subscribe, localLane.getSnapshot);
  const plex = useSyncExternalStore(plexLane.subscribe, plexLane.getSnapshot);
  const input = useRef<HTMLInputElement>(null), scroll = useRef<HTMLDivElement>(null);
  const position = useRef(0), origin = useRef<HTMLElement | null>(null);
  const resultButtons = useRef(new Map<string, HTMLButtonElement>());
  const keyboardHelp = useId();
  const showLocal = localConfigured && scope !== "plex", showPlex = plexConfigured && scope !== "local";
  const currentDetail = detail && detail.identity === (detail.row.source === "local" ? localDetailIdentity : plexIdentity) ? detail.row : null;

  useLayoutEffect(() => { localLane.clear(); }, [localLane, localIdentity, query, scope, retry]);
  useLayoutEffect(() => { plexLane.clear(); }, [plexLane, plexIdentity, query, scope, retry]);
  useLayoutEffect(() => { if (detail && !currentDetail) setDetail(null); }, [detail, currentDetail]);
  useEffect(() => {
    if (active && query && showLocal && isPlaybackAvailable && !catalog.loaded && !catalog.error) {
      localAlbumCatalog.ensureLoaded(status.is_updating, true);
    }
  }, [active, query, showLocal, isPlaybackAvailable, catalog.loaded, catalog.error, status.is_updating]);

  useEffect(() => {
    if (!active || !query || !showLocal || !localReady) { localLane.cancel(); return; }
    const timer = setTimeout(() => localLane.request(query, async current => {
      try {
        const page = await audioService.searchLocalTracks(query);
        return { rows: localTrackRows(page.tracks), limited: page.limited || page.tracks.length > SEARCH_PREVIEW_LIMIT };
      } catch (error) {
        if (current()) logError("search.local", error);
        return { rows: [], limited: false, tracksUnavailable: true };
      }
    }), 300);
    return () => { clearTimeout(timer); localLane.cancel(); };
  }, [active, query, showLocal, localReady, localIdentity, localIndex, localLane, retry]);

  useEffect(() => {
    if (!active || !query || !showPlex) { plexLane.cancel(); return; }
    const timer = setTimeout(() => plexLane.request(query, async () => {
      if (!config.plex_server_id) throw Error("SEARCH_CONTEXT_UNAVAILABLE");
      return plexSearchPreview(await plexService.search(query), config.plex_server_id);
    }), 300);
    return () => { clearTimeout(timer); plexLane.cancel(); };
  }, [active, query, showPlex, plexIdentity, plexLane, config.plex_server_id, retry]);
  useEffect(() => () => { localLane.cancel(); plexLane.cancel(); }, [localLane, plexLane]);

  useLayoutEffect(() => {
    if (!currentDetail && active && scroll.current) { scroll.current.scrollTop = position.current; origin.current?.focus({ preventScroll: true }); }
  }, [currentDetail, active]);
  useEffect(() => {
    if (!active) return;
    const focus = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || (event.target as Element | null)?.closest?.("dialog, [role=dialog], [role=alertdialog], [data-local-tag-dialog-open]")) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault(); setDetail(null); requestAnimationFrame(() => { input.current?.focus(); input.current?.select(); });
      }
    };
    const focusSearch = () => { setDetail(null); requestAnimationFrame(() => { input.current?.focus(); input.current?.select(); }); };
    window.addEventListener("sonante:focus-unified-search", focusSearch);
    window.addEventListener("keydown", focus); return () => { window.removeEventListener("keydown", focus); window.removeEventListener("sonante:focus-unified-search", focusSearch); };
  }, [active]);

  const playbackContext = useMemo(() => ({}), [localIdentity, plexIdentity, query, scope, kind, active, currentDetail, isPlaybackAvailable]);
  const request = usePlaybackAttempt(playbackContext);
  const queueActions = useQueueInsertion(playbackContext, active && !currentDetail && isPlaybackAvailable && !status.is_updating);
  const queueRequest = (row: Extract<SearchRow, { kind: "track" }>) => ({
    track: searchTrackMetadata(row), current: () => isCurrent(row),
    album: row.source === "local" ? () => loadQueuedAlbum(searchTrackMetadata(row))
      : row.track.album_rating_key ? async () => (await plexService.getAlbumTracks(row.track.album_rating_key!)).map(item => plexQueueTrack(item)) : undefined,
  });
  const live = useRef({ active, localReady, currentDetail, scope }); live.current = { active, localReady, currentDetail, scope };
  const isCurrent = (row: SearchRow) => live.current.active && !live.current.currentDetail
    && (live.current.scope === "all" || live.current.scope === row.source)
    && (row.source !== "local" || live.current.localReady)
    && (row.source === "local" && row.kind !== "track" ? localMetadataRef.current.rows.includes(row)
      : (row.source === "local" ? localLane : plexLane).getSnapshot().value?.rows.includes(row) === true);
  const open = (row: SearchRow, button: HTMLElement) => {
    if (!isCurrent(row)) return;
    if (row.kind === "track") {
      if (isPlaybackAvailable) void request.run(async intent => {
        if (isCurrent(row) && intent.current()) await audioService.playTracks([searchTrackMetadata(row)], 0, intent);
      });
    } else {
      origin.current = button; setDetail({ row, identity: row.source === "local" ? localDetailIdentity : plexIdentity });
    }
  };
  const rows = query ? sortSearchRows([
    ...(showLocal && localReady ? [...localMetadata.rows, ...local.value?.rows ?? []] : []), ...(showPlex ? plex.value?.rows ?? [] : []),
  ], query) : [];
  const selectionRows = rows.filter((row): row is Extract<SearchRow, { kind: "track" }> => row.kind === "track");
  const selection = useTrackSelection(playbackContext, selectionRows.map(searchTrackMetadata), active && !currentDetail && isPlaybackAvailable && !status.is_updating);

  const visibleRows = (["album", "artist", "track"] as const).flatMap(group =>
    kind === "all" || kind === group ? rows.filter(row => row.kind === group) : []);
  const enabledResults = () => visibleRows.map(row => resultButtons.current.get(row.key)).filter((button): button is HTMLButtonElement => Boolean(button && !button.disabled));
  const focusResult = (button: HTMLButtonElement | undefined) => {
    if (!button) return;
    button.focus({ preventScroll: true });
    button.scrollIntoView({ block: "nearest", inline: "nearest" });
  };
  const kinds = { all: t("unifiedSearch.everything"), album: t("plex.albums"), artist: t("plex.artists"), track: t("plex.tracks") };
  const chip = "rounded-full border px-4 py-1.5 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]";
  const selectedChip = "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#332b15)] text-[color:var(--theme-fg-accent,#E5A00D)]";
  const idleChip = "border-[color:var(--theme-border-soft,#333333)] text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:bg-[var(--theme-surface-raised,#262626)]";
  const localUnavailable = Boolean(catalog.error) || !isPlaybackAvailable || catalog.saveBlocked && !catalog.manualCheckPlayback;
  const searching = Boolean(query && (showLocal && !localUnavailable && (!localReady || local.phase === "idle" || local.phase === "loading")
    || showPlex && (plex.phase === "idle" || plex.phase === "loading")));
  const sourceStatus = (name: string, loading: boolean, failed: boolean, partial = false) => <span className="inline-flex items-center gap-2">
    {loading && <LoaderCircle size={13} className="animate-spin" aria-hidden="true" />}{name}: {t(failed ? "unifiedSearch.unavailable" : partial ? "unifiedSearch.partial" : loading ? "unifiedSearch.searching" : "unifiedSearch.ready")}
  </span>;
  const back = () => setDetail(null);

  return <div data-unified-search className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
    onKeyDown={event => {
      if (!active || event.defaultPrevented || event.nativeEvent?.isComposing || event.ctrlKey || event.metaKey || event.altKey
        || (event.target as Element | null)?.closest?.("dialog, [role=dialog], [role=alertdialog], [data-local-tag-dialog-open]")) return;
      if (event.key === "Escape" && (currentDetail || text)) {
        event.preventDefault(); event.stopPropagation();
        if (currentDetail) back();
        else { position.current = 0; origin.current = null; setText(""); input.current?.focus(); }
        return;
      }
      if (currentDetail) return; // Detail views own their keyboard navigation.
      const buttons = enabledResults();
      if (event.target === input.current) {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          if (buttons.length) { event.preventDefault(); focusResult(event.key === "ArrowDown" ? buttons[0] : buttons[buttons.length - 1]); }
        } else if (event.key === "Enter" && buttons.length) {
          event.preventDefault();
          if (!event.repeat) buttons[0].click();
        }
        return;
      }
      const index = buttons.indexOf(event.target as HTMLButtonElement);
      if (index < 0) return;
      if (event.key === "ArrowUp" && index === 0) { event.preventDefault(); input.current?.focus(); }
      else if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
        event.preventDefault();
        const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
          : Math.min(buttons.length - 1, index + (event.key === "ArrowDown" ? 1 : -1));
        focusResult(buttons[next]);
      }
    }}>
    <main className={currentDetail ? "hidden" : "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]"}>
      <header className="shrink-0 space-y-5 border-b border-[color:var(--theme-border-soft,#262626)] px-6 py-6 sm:px-8">
        <div><h1 className="flex items-center gap-3 text-2xl font-bold text-[color:var(--theme-fg-primary,#ffffff)]"><Search size={24} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />{t("unifiedSearch.title")}</h1>
          <p className="mt-1 text-sm text-[color:var(--theme-fg-muted,#888888)]">{t("unifiedSearch.description")}</p></div>
        <div className="relative max-w-3xl"><Search size={21} aria-hidden="true" className="absolute left-4 top-4 text-[color:var(--theme-fg-accent,#E5A00D)]" />
          <input ref={input} autoFocus type="search" maxLength={160} value={text} aria-label={t("unifiedSearch.inputLabel")} aria-describedby={keyboardHelp}
            placeholder={t("unifiedSearch.placeholder")} onChange={event => { position.current = 0; origin.current = null; setText(event.target.value); }}
            className="w-full rounded-xl border border-[color:var(--theme-border-soft,#3A3A3A)] bg-[var(--theme-surface-panel,#1A1A1A)] py-3.5 pl-12 pr-12 text-base text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] focus:ring-1 focus:ring-[color:var(--theme-border-accent,#E5A00D)]" />
          {text && <button type="button" aria-label={t("unifiedSearch.clear")} onClick={() => { setText(""); input.current?.focus(); }} className="absolute right-3 top-3 rounded-lg p-1.5 text-[color:var(--theme-fg-secondary,#AAAAAA)]"><X size={18} /></button>}
        </div>
        <p id={keyboardHelp} className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("unifiedSearch.keyboardHint")}</p>
        <div className="flex flex-wrap items-center gap-2" role="group" aria-label={t("unifiedSearch.sourceFilter")}>
          {(["all", ...(localConfigured ? ["local"] : []), ...(plexConfigured ? ["plex"] : [])] as SearchSource[]).map(item => <button key={item} type="button" aria-pressed={scope === item}
            onClick={() => { position.current = 0; setSource(item); }} className={`${chip} ${scope === item ? selectedChip : idleChip}`}>
            {item === "all" ? t("unifiedSearch.allSources") : t(item === "local" ? "explore.sources.local" : "explore.sources.plex")}</button>)}
          <span aria-hidden="true" className="mx-1 h-5 border-l border-[color:var(--theme-border-soft,#333333)]" />
          {(Object.keys(kinds) as SearchKind[]).map(item => <button key={item} type="button" aria-pressed={kind === item} onClick={() => setKind(item)} className={`${chip} ${kind === item ? selectedChip : idleChip}`}>{kinds[item]}</button>)}
        </div>
      </header>
      <div ref={scroll} onScroll={event => { if (!currentDetail) position.current = event.currentTarget.scrollTop; }} className="min-h-0 flex-1 overflow-y-auto px-6 py-6 sm:px-8">
        <PlaybackRequestNotice failure={request.failure} />
        {queueActions.notice}{queueActions.menu}{selection.toolbar}
        {!query || !localConfigured && !plexConfigured ? <div className="mx-auto max-w-xl py-16 text-center"><Search size={42} className="mx-auto mb-5 text-[color:var(--theme-fg-accent,#E5A00D)]" /><h2 className="text-lg font-semibold">{t("unifiedSearch.startTitle")}</h2><p className="mt-3 text-sm leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t(localConfigured || plexConfigured ? "unifiedSearch.startHint" : "unifiedSearch.configureHint")}</p></div> : <>
          <div role="status" className="mb-5 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-[color:var(--theme-fg-muted,#888888)]">
            {showLocal && sourceStatus(t("explore.sources.local"), !localUnavailable && (!localReady || local.phase === "loading" || local.phase === "idle"), localUnavailable || local.phase === "error", local.value?.tracksUnavailable)}
            {showPlex && sourceStatus(t("explore.sources.plex"), plex.phase === "loading" || plex.phase === "idle", plex.phase === "error")}
            <button type="button" onClick={() => setRetry(value => value + 1)} className="inline-flex items-center gap-1.5 rounded px-2 py-1 text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"><RefreshCw size={13} />{t("unifiedSearch.retry")}</button>
          </div>
          {(showLocal && (localUnavailable || local.phase === "error" || local.value?.tracksUnavailable) || showPlex && plex.phase === "error") && <p role="alert" className="mb-5 rounded-lg border border-[color:var(--theme-border-soft,#333333)] p-3 text-sm text-[color:var(--theme-fg-secondary,#AAAAAA)]">{t("unifiedSearch.partialError")}</p>}
          {(["album", "artist", "track"] as const).map(group => {
            if (kind !== "all" && kind !== group) return null;
            const entries = rows.filter(row => row.kind === group), Icon = group === "album" ? Disc3 : group === "artist" ? Users : Music2;
            if (!entries.length) return null;
            return <section key={group} aria-label={kinds[group]} className="mb-8"><h2 className="mb-3 flex items-center gap-2 text-base font-semibold"><Icon size={19} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />{kinds[group]}<span className="ml-1 rounded-full bg-[var(--theme-surface-raised,#262626)] px-2 py-0.5 text-xs text-[color:var(--theme-fg-muted,#888888)]">{entries.length}</span></h2>
              <div className="grid grid-cols-1 gap-2 xl:grid-cols-2">{entries.map(row => {
                const text = rowText(row), Origin = row.source === "local" ? Folder : Server;
                return <div key={row.key} className={`relative ${row.kind === "track" && selection.checked(selectionRows.indexOf(row)) ? "track-selected" : ""}`} onContextMenu={row.kind === "track" ? event => queueActions.open(event, queueRequest(row)) : undefined}>
                  <button type="button" data-search-result={row.key}
                  ref={button => { if (button) resultButtons.current.set(row.key, button); else resultButtons.current.delete(row.key); }} disabled={!active || row.kind === "track" && !isPlaybackAvailable}
                  title={[text.title, text.subtitle].filter(Boolean).join(" · ")}
                  onClick={event => { if (row.kind !== "track" || !selection.click(event, selectionRows.indexOf(row))) open(row, event.currentTarget); }} aria-label={`${row.kind === "track" ? t("unifiedSearch.playTrack") : t("unifiedSearch.open")}: ${text.title} · ${t(row.source === "local" ? "explore.sources.local" : "explore.sources.plex")}`}
                  className={`group flex w-full min-w-0 items-center gap-3 rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#181818)] px-4 py-3 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)]/60 hover:bg-[var(--theme-surface-raised,#222222)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:opacity-40 ${row.kind === "track" ? "pr-24" : ""}`}>
                  <span className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-[var(--theme-surface-accent,#332b15)] text-[color:var(--theme-fg-accent,#E5A00D)]"><Icon size={22} /></span>
                  <span className="min-w-0 flex-1"><span title={text.title} className="block truncate text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)]">{text.title}</span><span title={text.subtitle || undefined} className="mt-0.5 block truncate text-xs text-[color:var(--theme-fg-muted,#888888)]">{text.subtitle || t(row.kind === "artist" ? "unifiedSearch.artistAction" : "unifiedSearch.open")}</span></span>
                  <span className="flex shrink-0 items-center gap-1.5 rounded-full border border-[color:var(--theme-border-soft,#333333)] px-2.5 py-1 text-[11px] text-[color:var(--theme-fg-secondary,#AAAAAA)]"><Origin size={12} />{t(row.source === "local" ? "explore.sources.local" : "explore.sources.plex")}</span>
                  {row.kind === "track" ? <Play size={17} className="shrink-0 text-[color:var(--theme-fg-accent,#E5A00D)]" /> : <ArrowUpRight size={17} className="shrink-0 text-[color:var(--theme-fg-muted,#888888)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)]" />}
                </button>{row.kind === "track" && <span className="absolute right-2 top-1/2 flex items-center gap-2 -translate-y-1/2">{selection.checkbox(selectionRows.indexOf(row))}{queueActions.trigger(queueRequest(row))}</span>}</div>;
              })}</div></section>;
          })}
          {!searching && !rows.some(row => kind === "all" || row.kind === kind) && <p className="py-12 text-center text-sm text-[color:var(--theme-fg-muted,#888888)]">{t("unifiedSearch.empty")}</p>}
          {(localMetadata.limited || local.value?.limited || plex.value?.limited) && <p className="mt-5 text-xs text-[color:var(--theme-fg-muted,#888888)]">{t("unifiedSearch.previewHint")}</p>}
        </>}
      </div>
    </main>
    {currentDetail && <div className="flex min-h-0 min-w-0 flex-1 flex-col">{currentDetail.source === "local" && <div className="shrink-0 border-b border-[color:var(--theme-border-soft,#262626)] px-6 py-3 sm:px-8"><button type="button" onClick={back} className="flex items-center gap-2 rounded-lg px-2 py-1 text-sm text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"><ArrowLeft size={17} />{t("unifiedSearch.back")}</button></div>}
      {currentDetail.source === "local" ? <LocalBrowserView key={currentDetail.key} externalBackNavigation
        initialAlbumId={currentDetail.kind === "album" ? currentDetail.album.id : undefined}
        initialArtist={currentDetail.kind === "artist" ? currentDetail.artist.name : undefined} onClearInitialArtist={back}
        onAlbumNavigationBack={back} catalogRefreshRevision={catalogRefreshRevision} isPlaybackAvailable={isPlaybackAvailable}
        isLibraryUpdating={status.is_updating} onlineArtworkEnabled={config.online_artwork_enabled} configuredLocalFolders={config.local_folders} />
      : currentDetail.kind === "album" ? <AlbumView album={currentDetail.album} active={active} onBack={back} backLabel={t("unifiedSearch.back")} isPlaybackAvailable={isPlaybackAvailable}
        onToggleFavorite={onFavoritesChanged} onSelectArtist={artist => setDetail({ row: { source: "plex", kind: "artist", key: `plex:artist:${artist.rating_key}`, artist: { rating_key: artist.rating_key, name: artist.name, thumb: artist.thumb ?? undefined } }, identity: plexIdentity })} />
      : <ArtistView artist={{ ...currentDetail.artist, server_id: config.plex_server_id ?? undefined }} requestIdentity={plexIdentity} active={active} onBack={back} backLabel={t("unifiedSearch.back")}
        onSelectAlbum={album => setDetail({ row: { source: "plex", kind: "album", key: `plex:album:${album.rating_key}`, album }, identity: plexIdentity })}
        status={status} isPlaybackAvailable={isPlaybackAvailable} />}
    </div>}
  </div>;
};
