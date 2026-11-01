import { AlbumCatalog } from "./AlbumCatalog";
import { LocalCatalogReadError } from "./LocalCatalogReadError";
import { AlbumCatalogRow } from "./AlbumCatalogRow";
import { AlbumViewControls } from "./AlbumViewControls";
import { readAlbumPresentation, saveAlbumPresentation, type AlbumLayout, type AlbumPresentation } from "../utils/albumPresentation";
import { useTrackSelection } from "./TrackSelection";
import { loadLocalAlbumQueue } from "../utils/queueAlbumLoader";
import { useQueueInsertion } from "./QueueInsertionMenu";
import { localQueueTrack } from "../utils/queueActions";
import { loadQueuedAlbum } from "../utils/queueAlbumLoader";
import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useLayoutEffect, useMemo, useState, useRef, useSyncExternalStore } from "react";
import { createPlayerAlbumLookup, type PlayerNavigationTarget } from "../utils/playerNavigation";
import {
  Folder,
  Play,
  ArrowLeft,
  Disc3,
  Search,
  Grid,
  ListTree,
  Users,
  CalendarDays,
  Tags,
  Clock,
  Sparkles,
  RefreshCw,
  Heart,
  ListPlus,
  Pencil,
  Info,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { LocalImportPreviewSection } from "./LocalImportPreviewSection";
import { filterImportPreview, importPreviewForCatalog } from "../utils/localImportPreview";
import type { ImportArtworkCache } from "../utils/importArtworkCache";
import type { PreviewAlbum } from "../types/localImportPreview";
import { LocalItem, LocalAlbum } from "../types/local";
import { FavoriteAlbum } from "../types/favorite";
import { audioService } from "../services/audio";
import { favoritesService } from "../services/favorites";
import { PlaylistPickerModal } from "./PlaylistPickerModal";
import type { NewPlaylistItem } from "../types/playlist";
import { flattenAlbumDiscs, type AlbumDiscTracks } from "../utils/localAlbumDiscs";
import { localAlbumCatalog } from "../utils/localAlbumCatalogSession";
import { confirmedCatalogPlaybackAllowed, emptyLocalAlbumCatalogState, refreshCatalogAfterSuccessfulCheck, localCatalogActivity, canRetryLocalCatalog, localCatalogPresentationAlbums } from "../utils/localAlbumCatalog";
import { albumLocationSources } from "../utils/localAlbumLocations";
import { localArtworkEnrichment, localArtworkResolver, localAlbumThumbnails } from "../utils/localArtworkSession";
import {
  filterLocalAlbumsByScope,
  localLibrarySourceOptions,
  localLibraryPathLabel,
  localLibrarySelectorPresentation,
  validLocalLibrarySourceSelection,
} from "../utils/localLibraryScope";
import {
  isTextLocalAlbumSort,
  LOCAL_ALBUM_INDEX_BUCKETS,
  localAlbumBucketFirstIndices,
  localAlbumNavigationResetKey,
  sortLocalAlbums,
  type LocalAlbumSortMode,
} from "../utils/localAlbumNavigation";
import {
  albumsForLocalArtist,
  deriveLocalArtists,
  filterAndSortLocalArtists,
  localArtistBucketFirstIndices,
  type LocalArtistSortMode,
} from "../utils/localArtistNavigation";
import {
  deriveLocalYearNavigation,
  filterLocalAlbumsByYear,
  filterLocalAlbumsWithoutYear,
  validLocalYearSelection,
} from "../utils/localYearNavigation";
import { type VirtualAlbumGridHandle } from "./VirtualAlbumGrid";
import { VirtualList, type VirtualListHandle } from "./VirtualList";
import { AlbumArtworkPlaceholder } from "./AlbumArtworkPlaceholder";
import { ToolbarSelect } from "./ToolbarSelect";
import { LocalMetadataView } from "./LocalMetadataView";
import { LocalAlbumTagEditorModal } from "./LocalAlbumTagEditorModal";
import { LibraryDoctorModal } from "./LibraryDoctorModal";
import { libraryDoctorAvailable } from "../utils/libraryDoctor";
import type { DoctorReviewContext } from "../utils/libraryDoctorReview";
import { LocalFileInfoModal } from "./LocalFileInfoModal";
import { LocalTagEditorModal } from "./LocalTagEditorModal";
import type { LocalTrackTags } from "../types/localTags";
import { createAlbumTracksLoader, type AlbumTracksState } from "../utils/localAlbumTracks";
import { artworkReadBlocked } from "../utils/localArtwork";
import { validAlbumGenre } from "../utils/albumGenres";
import { deriveLocalGenreFamilies, filterLocalAlbumsByFamily } from "../utils/localGenreFamilies";
import { AlbumGenreGrid } from "./AlbumGenreGrid";

export { localAlbumCatalog } from "../utils/localAlbumCatalogSession";

async function getLocalAlbumCover(album: LocalAlbum, onlineEnabled: boolean | (() => boolean), libraryUpdating: boolean | (() => boolean), consumerActive: () => boolean = () => true): Promise<string | null> {
  const version = localAlbumCatalog.getSnapshot().albums;
  return localArtworkResolver.resolve(album, onlineEnabled, () => {
    const catalog = localAlbumCatalog.getSnapshot();
    const updating = typeof libraryUpdating === "function" ? libraryUpdating() : libraryUpdating;
    return artworkReadBlocked(updating, catalog) || catalog.albums !== version || !catalog.albums.includes(album);
  }, undefined, consumerActive);
}

async function loadAlbumDiscTracks(album: LocalAlbum): Promise<AlbumDiscTracks[]> {
  return audioService.getLocalAlbumTracks(album.id);
}

const LocalAlbumCard: React.FC<{
  layout?: AlbumLayout;
  album: LocalAlbum;
  isFavorite: boolean;
  onToggleFavorite: (e: React.MouseEvent, album: LocalAlbum, cover: string | null) => void;
  onClick: () => void;
  onPlayQuick: (e: React.MouseEvent) => void;
  isPlaybackAvailable: boolean;
  onlineArtworkEnabled: boolean;
  isLibraryUpdating: boolean;
  readEnabled: boolean;
}> = ({ layout = "grid", album, isFavorite, onToggleFavorite, onClick, onPlayQuick, isPlaybackAvailable, onlineArtworkEnabled, isLibraryUpdating, readEnabled }) => {
  const { t } = useTranslation();
  const artworkEnabled = layout !== "details";
  const cardContext = useMemo(() => ({}), [album, readEnabled, layout]);
  const cardActions = useQueueInsertion(cardContext, readEnabled && isPlaybackAvailable);
  const albumRequest = { track: { title: album.title, artist: album.artist, album: album.title }, scope: "album" as const, load: () => loadLocalAlbumQueue(album) };
  const [cover, setCover] = useState<string | null>(() => readEnabled && artworkEnabled ? localArtworkResolver.get(album.id) : null);
  const [failedCover, setFailedCover] = useState<string | null>(null);

  const cardRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const [visible, setVisible] = useState(false);
  const [coverChecked, setCoverChecked] = useState(false);
  const displayCover = artworkEnabled ? (readEnabled ? cover : null) || (!readEnabled || !coverChecked ? localAlbumThumbnails.get(album) : null) : null;
  const readEnabledRef = useRef(readEnabled);
  readEnabledRef.current = readEnabled;
  const onlineEnabledRef = useRef(onlineArtworkEnabled);
  const libraryUpdatingRef = useRef(isLibraryUpdating);
  onlineEnabledRef.current = onlineArtworkEnabled;
  libraryUpdatingRef.current = isLibraryUpdating;

  useEffect(() => {
    setCoverChecked(false);
    if (!readEnabled || !artworkEnabled) { setCover(null); return; }
    setCover(localArtworkResolver.get(album.id));
    return localArtworkResolver.subscribe(album.id, setCover);
  }, [album, readEnabled, artworkEnabled]);

  useEffect(() => {
    if (!artworkEnabled) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "150px" }
    );

    if (cardRef.current) {
      observer.observe(cardRef.current);
    }

    return () => {
      observer.disconnect();
    };
  }, [album.folder_path, artworkEnabled]);

  useEffect(() => {
    if (!readEnabled || !artworkEnabled || !visible || cover) return;
    let active = true;
    const version = localAlbumCatalog.getSnapshot().albums;
    const current = () => {
      const catalog = localAlbumCatalog.getSnapshot();
      return active && readEnabledRef.current && catalog.albums === version && catalog.albums.includes(album)
        && !artworkReadBlocked(libraryUpdatingRef.current, catalog);
    };
    getLocalAlbumCover(album, () => onlineEnabledRef.current, () => libraryUpdatingRef.current, () => active && readEnabledRef.current)
      .then((found) => {
        if (!active) return;
        if (found) { setCover(found); setCoverChecked(true); }
        else if (current()) { setCoverChecked(true); localAlbumThumbnails.forget(album, current); }
      })
      .catch((error) => logError("artwork.album-load", error));
    return () => { active = false; };
  }, [album, visible, cover, onlineArtworkEnabled, isLibraryUpdating, readEnabled, artworkEnabled]);

  const captureResolvedImage = (image: HTMLImageElement, trigger: "on_load" | "already_complete") => {
    if (!artworkEnabled || !readEnabled || !cover || displayCover !== cover || (image.currentSrc || image.src) !== cover) return;
    localAlbumThumbnails.capture(album, image, () => {
      const current = localAlbumCatalog.getSnapshot();
      return current.albums.includes(album) && !artworkReadBlocked(libraryUpdatingRef.current, current);
    }, trigger);
  };

  // Data images may already be complete when catalog admission changes or React
  // reuses the DOM node. Such a transition does not promise another load event.
  useEffect(() => {
    const image = imageRef.current;
    if (image?.complete) captureResolvedImage(image, "already_complete");
  }, [album, cover, displayCover, readEnabled, isLibraryUpdating]);

  useEffect(() => { if (artworkEnabled) localAlbumThumbnails.observeInitialCard(album); }, [album, artworkEnabled]);

  const artwork = displayCover && displayCover !== failedCover ? (
          <img
            src={displayCover}
            alt={album.title}
            className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
            loading="lazy"
            ref={imageRef}
            onLoad={event => captureResolvedImage(event.currentTarget, "on_load")}
            onError={() => setFailedCover(displayCover)}
          />
        ) : (
          <AlbumArtworkPlaceholder />
        );
  if (layout !== 'grid') return <div ref={cardRef} onContextMenu={event => cardActions.open(event, albumRequest)} className="relative h-full min-w-0">
    {cardActions.menu}{cardActions.notice && <div className="absolute inset-x-0 bottom-0 z-20">{cardActions.notice}</div>}
    <AlbumCatalogRow title={album.title} artist={album.artist} year={album.year} trackCount={album.track_count}
      discCount={album.discs.length} genres={album.genres} library={album.source_label} addedAt={album.added_at}
      detailed={layout==='details'} artwork={layout==='list'?artwork:undefined} favorite={isFavorite}
      readable={readEnabled} playable={readEnabled&&isPlaybackAvailable} onOpen={onClick} onPlay={onPlayQuick}
      onFavorite={event=>onToggleFavorite(event,album,cover)} queueAction={cardActions.trigger(albumRequest)} />
  </div>;

  return (
    <div ref={cardRef} onContextMenu={event => cardActions.open(event, albumRequest)} onClick={readEnabled ? onClick : undefined} aria-disabled={!readEnabled} className={`group flex flex-col relative ${readEnabled ? "cursor-pointer" : "cursor-default"}`}>
      {cardActions.menu}{cardActions.notice}
      <div className="relative aspect-square w-full rounded-lg bg-[var(--theme-surface-raised,#202020)] overflow-hidden mb-2.5 shadow-md">
        <span className="album-queue-trigger">{cardActions.trigger(albumRequest)}</span>
        {artwork}


        <button data-theme-surface="contrast"
          onClick={(e) => onToggleFavorite(e, album, cover)}
          disabled={!readEnabled}
          className={`absolute top-2 right-2 p-1.5 rounded-full backdrop-blur-xs transition-transform active:scale-90 cursor-pointer shadow z-10 ${
            isFavorite
              ? "bg-black/60 text-[color:var(--theme-fg-accent,#E5A00D)]"
              : "bg-[var(--theme-surface-subtle,rgb(0_0_0_/_0.4))] text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.7))] hover:text-[color:var(--theme-fg-primary,#ffffff)] opacity-0 group-hover:opacity-100"
          }`}
        >
          <Heart size={14} fill={isFavorite ? "#E5A00D" : "none"} />
        </button>

        <div data-theme-surface="contrast" className="absolute inset-0 bg-[var(--theme-surface-subtle,rgb(0_0_0_/_0.4))] opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
          <button
            onClick={onPlayQuick}
            disabled={!isPlaybackAvailable || !readEnabled}
            className="w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Play size={20} className="ml-1" fill="black" />
          </button>
        </div>
      </div>

      <span className="text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)] truncate" title={album.title}>
        {album.title}
      </span>
      <span className="text-xs text-[color:var(--theme-fg-secondary,#999999)] truncate mt-0.5" title={album.artist}>
        {album.artist}
      </span>
      <span className="text-[11px] text-[color:var(--theme-fg-muted,#666666)] mt-0.5">
        {album.year ? `${album.year} • ` : ""}
        {t("localBrowser.trackCount", { count: album.track_count })}
      </span>
    </div>
  );
};

export type LocalSubview = "browser" | "metadata";

export interface LocalBrowserViewProps {
  subview?: LocalSubview;
  onSubviewChange?: (subview: LocalSubview) => void;
  initialAlbumId?: string;
  albumNavigationAvailable?: boolean;
  onAlbumNavigationBack?: () => void;
  externalBackNavigation?: boolean;
  playerTarget?: Extract<PlayerNavigationTarget, { kind: "local" }>;
  onPlayerNavigationBack?: () => void;
  importPreview?: readonly PreviewAlbum[];
  importPreviewRevision?: number | null;
  importArtworkCache?: ImportArtworkCache;
  initialArtist?: string | null;
  onClearInitialArtist?: () => void;
  catalogRefreshRevision: number;
  isPlaybackAvailable: boolean;
  isLibraryUpdating: boolean;
  onlineArtworkEnabled: boolean;
  configuredLocalFolders: readonly string[];
}

export const LocalBrowserView: React.FC<LocalBrowserViewProps> = ({
  subview = "browser",
  onSubviewChange,
  initialAlbumId,
  albumNavigationAvailable = true,
  onAlbumNavigationBack,
  externalBackNavigation = false,
  playerTarget,
  onPlayerNavigationBack,
  importPreview = [],
  importPreviewRevision = null,
  importArtworkCache,
  initialArtist,
  onClearInitialArtist,
  catalogRefreshRevision,
  isPlaybackAvailable,
  isLibraryUpdating,
  onlineArtworkEnabled,
  configuredLocalFolders,
}) => {
  const { t } = useTranslation();
  const catalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const artworkActivity = useSyncExternalStore(localArtworkEnrichment.subscribe, localArtworkEnrichment.getOnlineActivitySnapshot);
  const albums = localCatalogPresentationAlbums(catalog);
  const openingAlbums = useMemo(() => sortLocalAlbums(albums, "album-asc"), [albums]);
  useLayoutEffect(() => { localAlbumThumbnails.setOpeningOrder(openingAlbums); }, [openingAlbums]);
  const initialAlbum = useMemo(() => initialAlbumId ? albums.find(album => album.id === initialAlbumId) ?? null : null, [initialAlbumId, albums]);
  const detailUnavailable = !!initialAlbumId && (!albumNavigationAvailable || !initialAlbum);
  const metadataMode = subview === "metadata" || detailUnavailable;
  const browserReads = useRef(!metadataMode);
  browserReads.current = !metadataMode;
  const [metadataAlbumId, setMetadataAlbumId] = useState<string | null>(playerTarget?.albumId ?? initialAlbumId ?? null);
  const playerAlbum = useMemo(() => playerTarget
    ? albums.find(item => item.id === playerTarget.albumId) ?? null : null, [playerTarget, albums]);
  const [viewMode, setViewMode] = useState<"albums" | "artists" | "years" | "genres" | "folders">(playerTarget?.view === "artist" ? "artists" : "albums");
  const emptyCatalogState = emptyLocalAlbumCatalogState(catalog, isLibraryUpdating);
  const catalogActivity = localCatalogActivity(isLibraryUpdating, catalog);
  const [albumSearch, setAlbumSearch] = useState("");
  const [selectedGenre, setSelectedGenre] = useState<string | null>(null);
  const [artistSearch, setArtistSearch] = useState("");
  const [selectedSourceId, setSelectedSourceId] = useState<string | null>(playerAlbum?.source_id ?? initialAlbum?.source_id ?? null);
  const [albumPresentation, setAlbumPresentation] = useState<AlbumPresentation>(()=>readAlbumPresentation("local"));
  const changeAlbumPresentation = (next: AlbumPresentation) => { setAlbumPresentation(next); saveAlbumPresentation("local", next); };
  const [albumSortMode, setAlbumSortMode] = useState<LocalAlbumSortMode>("album-asc");
  const [artistSortMode, setArtistSortMode] = useState<LocalArtistSortMode>("artist-asc");
  const [selectedDecade, setSelectedDecade] = useState<number | null>(null);
  const [selectedYear, setSelectedYear] = useState<number | null>(null);
  const [unknownYearSelected, setUnknownYearSelected] = useState(false);
  const [selectedAlbum, setSelectedAlbum] = useState<LocalAlbum | null>(playerTarget?.view === "album" ? playerAlbum : initialAlbum);
  const [albumLocations, setAlbumLocations] = useState<{
    album: LocalAlbum;
    paths: { label: string | null; absolutePath: string }[];
  } | null>(null);
  const [selectedArtist, setSelectedArtist] = useState<string | null>(playerTarget?.view === "artist" ? playerAlbum?.artist ?? null : initialArtist || null);
  const [trackState, setTrackState] = useState<AlbumTracksState>({ albumId: null, phase: "waiting", groups: [], error: null, catalogVersion: null });
  const tracksLoader = useMemo(() => createAlbumTracksLoader(createPlayerAlbumLookup(
    playerTarget, () => localAlbumCatalog.getSnapshot().albums, audioService.getLocalAlbumTracks,
  ), setTrackState, () => localAlbumCatalog.waitForMpd()), []);
  const catalogConfirmed = !detailUnavailable && confirmedCatalogPlaybackAllowed(catalog, isLibraryUpdating);
  // A check permits playback of confirmed tracks, not editing/inspecting a
  // generation while its writer owns it. Keep those existing guards strict.
  const catalogInspectionAllowed = catalogConfirmed && !isLibraryUpdating && !catalog.manualCheckPlayback;
  const artworkBlocked = metadataMode || artworkReadBlocked(isLibraryUpdating, catalog);
  const tracksReady = catalogConfirmed && trackState.catalogVersion === albums && trackState.phase === "ready" && trackState.albumId === selectedAlbum?.id;
  const { tracks: albumTracks, sections: albumSections } = useMemo(
    () => flattenAlbumDiscs(tracksReady ? trackState.groups : []), [tracksReady, trackState.groups],
  );
  const [albumCover, setAlbumCover] = useState<string | null>(null);
  const albumGridRef = useRef<VirtualAlbumGridHandle>(null);
  const artistListRef = useRef<VirtualListHandle>(null);
  const onlineEnabledRef = useRef(onlineArtworkEnabled);
  const libraryUpdatingRef = useRef(isLibraryUpdating);
  const artworkBlockedRef = useRef(artworkBlocked);
  artworkBlockedRef.current = artworkBlocked;
  onlineEnabledRef.current = onlineArtworkEnabled && !catalog.manualCheckPlayback;
  libraryUpdatingRef.current = isLibraryUpdating;
  const initialCatalogMountHandled = useRef(false);
  const observedCatalogRefreshRevision = useRef(catalogRefreshRevision);
  const [favoriteIds, setFavoriteIds] = useState<Set<string>>(new Set());
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);
  const [tagEditorAlbum, setTagEditorAlbum] = useState<LocalAlbum | null>(null);
  const [doctorReviewContext, setDoctorReviewContext] = useState<DoctorReviewContext | null>(null);
  const [doctorAlbum, setDoctorAlbum] = useState<LocalAlbum | null>(null);
  const doctorSelectedAlbum = useRef<string | undefined>(undefined);
  // Keep the dialog's identity stable across Local subviews; selection is blocked while it is open.
  doctorSelectedAlbum.current = doctorAlbum?.id ?? (metadataMode ? metadataAlbumId ?? undefined : selectedAlbum?.id);
  const libraryUpdatingForTags = useRef(isLibraryUpdating);
  libraryUpdatingForTags.current = isLibraryUpdating;
  const albumTagEditingAvailable = (albumId: string) => {
    const current = localAlbumCatalog.getSnapshot();
    return !detailUnavailable && current.loaded && !current.loading && !current.error && !current.mpdBusy
      && !current.saveBlocked && !current.manualCheckPlayback && !libraryUpdatingForTags.current
      && current.albums.some((album) => album.id === albumId);
  };
  const [tagEditorTrack, setTagEditorTrack] = useState<LocalItem | null>(null);
  const [fileInfoTrack, setFileInfoTrack] = useState<{ track: LocalItem; catalog: ReturnType<typeof localAlbumCatalog.getSnapshot> } | null>(null);
  const tagEditorOpen = useRef(false);
  tagEditorOpen.current = Boolean(tagEditorAlbum || tagEditorTrack || fileInfoTrack);
  const [scrollContainer, setScrollContainer] = useState<HTMLDivElement | null>(null);

  const [currentPath, setCurrentPath] = useState<string>("");
  const [items, setItems] = useState<LocalItem[]>([]);
  const [loadingFolders, setLoadingFolders] = useState<boolean>(false);
  const [folderCover, setFolderCover] = useState<string | null>(null);

  useEffect(() => {
    let isMounted = true;
    favoritesService
      .getFavorites()
      .then((favs) => {
        if (isMounted) {
          setFavoriteIds(new Set(favs.filter((f) => f.source === "local").map((f) => f.id)));
        }
      })
      .catch(error => logError("favorites.load", error));

    return () => {
      isMounted = false;
    };
  }, []);

  useEffect(() => {
    if (!initialCatalogMountHandled.current) {
      initialCatalogMountHandled.current = true;
      // A player destination uses the already loaded catalogue; it must not rebuild it.
      if (initialAlbumId) return;
      // Artist destinations also come from the shared catalog. Load only if
      // missing, preserving the same readiness and source-admission checks.
      if (initialArtist) {
        localAlbumCatalog.ensureLoaded(isLibraryUpdating, isPlaybackAvailable);
        return;
      }
      if (metadataMode && localAlbumCatalog.getSnapshot().loaded) return;
      if (!playerTarget) localAlbumCatalog.setUpdating(isLibraryUpdating, true);
    }
  }, []);

  useEffect(() => {
    localAlbumCatalog.setUpdating(isLibraryUpdating);
  }, [isLibraryUpdating]);

  useEffect(() => {
    if (observedCatalogRefreshRevision.current === catalogRefreshRevision) return;
    observedCatalogRefreshRevision.current = catalogRefreshRevision;
    localAlbumCatalog.requestRefresh();
  }, [catalogRefreshRevision]);

  useEffect(() => {
    if (!artworkBlocked && !initialAlbumId) {
      localArtworkEnrichment.setCatalog(catalog.albums);
    }
  }, [catalog.albums, artworkBlocked, initialAlbumId]);

  useEffect(() => {
    if (!selectedAlbum || metadataMode) return;
    const cached = localArtworkResolver.get(selectedAlbum.id);
    if (cached) setAlbumCover(cached);
    return localArtworkResolver.subscribe(selectedAlbum.id, setAlbumCover);
  }, [selectedAlbum, metadataMode]);

  useEffect(() => {
    if (!selectedAlbum || artworkBlocked) return;
    let active = true;
    void getLocalAlbumCover(selectedAlbum, () => onlineEnabledRef.current, () => artworkBlockedRef.current, () => active && browserReads.current)
      .then(cover => { if (active && cover) setAlbumCover(cover); })
      .catch(error => logError("artwork.album-load", error));
    return () => { active = false; };
  }, [selectedAlbum, artworkBlocked]);

  useEffect(() => {
    if (initialArtist) {
      setSelectedArtist(initialArtist);
      setSelectedAlbum(null);
      setViewMode("artists");
    }
  }, [initialArtist]);

  useEffect(() => {
    if (!playerTarget) return;
    const album = playerAlbum;
    if (!album) {
      // Keep an in-flight batch and its results mounted until the user closes the editor.
      if (!tagEditorAlbum && !tagEditorTrack && !fileInfoTrack && !doctorAlbum && !metadataMode) onPlayerNavigationBack?.();
      return;
    }
    if (!browserReads.current) return;
    setSelectedSourceId(album.source_id ?? null);
    setSelectedAlbum(playerTarget.view === "album" ? album : null);
    setSelectedArtist(playerTarget.view === "artist" ? album.artist : null);
    setViewMode(playerTarget.view === "artist" ? "artists" : "albums");
  }, [playerTarget, playerAlbum, tagEditorAlbum, fileInfoTrack]);

  useEffect(() => {
    if (!selectedAlbum || !catalog.loaded || metadataMode || albumLocations?.album === selectedAlbum) return;
    let active = true;
    const album = selectedAlbum;
    const sources = albumLocationSources(album);
    void Promise.allSettled(
      sources.map((source) => audioService.resolveLocalLibraryPath(source.path)),
    ).then((results) => {
      if (!active) return;
      const paths = results.flatMap((result, index) => {
        if (result.status === "rejected") {
          logError("local.album-location", result.reason);
          return [];
        }
        return [{ label: sources[index].label, absolutePath: result.value }];
      });
      setAlbumLocations({ album, paths });
    });
    return () => {
      active = false;
    };
  }, [selectedAlbum, metadataMode, catalog.loaded]);

  useEffect(() => {
    let active = true;
    if (!metadataMode && viewMode === "folders") {
      setLoadingFolders(true);
      audioService.listLocalDirectory(currentPath)
        .then((data) => {
          if (active) setItems(data);
        })
        .catch(error => { if (active) logError("local.directory-list", error); })
        .finally(() => { if (active) setLoadingFolders(false); });
    }
    return () => { active = false; };
  }, [currentPath, viewMode, metadataMode]);

  useEffect(() => { setFolderCover(null); }, [currentPath]);
  useEffect(() => {
    if (viewMode !== "folders" || artworkBlocked) return;
    let active = true;
    const version = catalog.albums;
    void audioService.getLocalCover(currentPath).then(cover => {
      const currentCatalog = localAlbumCatalog.getSnapshot();
      if (active && currentCatalog.albums === version && !artworkReadBlocked(libraryUpdatingRef.current, currentCatalog)) setFolderCover(cover);
    }).catch(error => logError("artwork.folder-load", error));
    return () => { active = false; };
  }, [currentPath, viewMode, artworkBlocked, catalog.albums]);

  const handleToggleFavoriteLocal = async (e: React.MouseEvent, album: LocalAlbum, cov: string | null) => {
    e.stopPropagation();
    const current = localAlbumCatalog.getSnapshot();
    if (!current.loaded || !current.albums.includes(album)) return;
    const favItem: FavoriteAlbum = {
      id: album.folder_path,
      source: "local",
      title: album.title,
      artist: album.artist,
      year: album.year,
      thumb: cov,
      path_or_key: album.folder_path,
      exists: true,
    };
    try {
      const added = await favoritesService.toggleFavorite(favItem);
      setFavoriteIds((prev) => {
        const next = new Set(prev);
        if (added) next.add(album.folder_path);
        else next.delete(album.folder_path);
        return next;
      });
    } catch (err) {
      logError("local.favorite-toggle", err);
    }
  };

  const handleSelectAlbum = async (album: LocalAlbum) => {
    if (!localAlbumCatalog.getSnapshot().loaded) return;
    setSelectedAlbum(album);
    setAlbumLocations(null);
    setAlbumCover(null);
  };

  useEffect(() => {
    if (metadataMode) return;
    if (trackState.phase === "ready" && trackState.albumId === selectedAlbum?.id && trackState.catalogVersion === albums && catalogConfirmed) return;
    const exists = selectedAlbum && albums.some(album => album.id === selectedAlbum.id);
    tracksLoader.select(exists ? selectedAlbum.id : null, catalogConfirmed, albums);
    return () => tracksLoader.cancel();
  }, [selectedAlbum?.id, albums, catalogConfirmed, tracksLoader, metadataMode]);

  useEffect(() => {
    if (!selectedAlbum) return;
    const refreshedAlbum = albums.find((album) => album.id === selectedAlbum.id);
    if (!refreshedAlbum) {
      setSelectedAlbum(null);
    } else if (refreshedAlbum !== selectedAlbum) {
      void handleSelectAlbum(refreshedAlbum);
    }
  }, [albums]);

  // Feedback belongs to one attempt in one Local view; old replies cannot
  // reappear after navigation, a new attempt or unmount.
  const playbackContext = useMemo(() => ({}), [
    subview, viewMode, currentPath, selectedAlbum?.id, selectedArtist, selectedSourceId,
    selectedGenre, selectedDecade, selectedYear, unknownYearSelected, albumSearch, artistSearch,
    initialAlbumId, playerTarget, albums, catalogRefreshRevision, configuredLocalFolders,
    isPlaybackAvailable, catalogConfirmed,
  ]);
  const localPlayback = usePlaybackAttempt(playbackContext, "local.album-play");
  const runLocalPlayback = localPlayback.run;
  const playbackFailure = localPlayback.failure;
  const queueActions = useQueueInsertion(playbackContext, isPlaybackAvailable && catalogConfirmed && !isLibraryUpdating && !catalog.manualCheckPlayback);
  const selectableTracks = useMemo(() => (selectedAlbum ? albumTracks : items.filter(item => item.item_type !== "directory")).map(track => localQueueTrack(track, selectedAlbum, selectedAlbum ? albumCover : folderCover)), [selectedAlbum, albumTracks, items, albumCover, folderCover]);
  const selection = useTrackSelection(playbackContext, selectableTracks, isPlaybackAvailable && catalogConfirmed && !isLibraryUpdating && !catalog.manualCheckPlayback);
  const folderSelectionIndices = new Map(items.filter(item => item.item_type !== "directory").map((item, index) => [item.path, index]));
  const queueRequest = (track: LocalItem, album?: LocalAlbum | null) => ({
    track: localQueueTrack(track, album, album ? albumCover : folderCover),
    album: album && tracksReady ? async () => albumTracks.map(item => localQueueTrack(item, album, albumCover)) : () => loadQueuedAlbum(localQueueTrack(track, album)),
  });

  const handlePlayEntireAlbum = (album: LocalAlbum, trackItems?: LocalItem[], startIdx = 0) => runLocalPlayback(async intent => {
    if (!isPlaybackAvailable || !catalogConfirmed || (trackItems && !tracksReady)) return;
    const files = trackItems ?? flattenAlbumDiscs(await loadAlbumDiscTracks(album)).tracks;
    const cov = (selectedAlbum?.id === album.id ? albumCover : null) || await localArtworkResolver.resolveLocalOnly(album, () => artworkReadBlocked(libraryUpdatingRef.current, localAlbumCatalog.getSnapshot())).catch(error => {
      // Optional artwork must not reject a valid playback request when the
      // catalog writer owns its read scope. Native music/queue errors still fail.
      logError("local.album-artwork", error);
      return null;
    });
    const currentCatalog = localAlbumCatalog.getSnapshot();
    if (currentCatalog.albums !== albums || !confirmedCatalogPlaybackAllowed(currentCatalog, libraryUpdatingRef.current)) return;
    const meta = files.map((f) => ({
      title: f.title || f.name,
      artist: f.artist || album.artist,
      album: f.album || album.title,
      thumb: cov,
      uri: f.path,
	duration: f.duration,
    }));
    if (meta.length > 0) {
      await audioService.playTracks(meta, startIdx, intent);
    }
  });

  const formatDuration = (secs?: number) => {
    if (!secs || isNaN(secs)) return "--:--";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const toPlaylistItem = (
    track: LocalItem,
    fallbackAlbum?: LocalAlbum,
  ): NewPlaylistItem => ({
    media_locator: { kind: "local", uri: track.path },
    metadata: {
      title: track.title || track.name,
      artist: track.artist || fallbackAlbum?.artist || "",
      album: track.album || fallbackAlbum?.title || "",
      duration: track.duration,
    },
  });

  const handleLocalTagsSaved = (path: string, tags: LocalTrackTags) => {
    const updateTrack = (track: LocalItem): LocalItem => track.path === path
      ? {
          ...track,
          title: tags.title ?? undefined,
          artist: tags.artist ?? undefined,
          album: tags.album ?? undefined,
        }
      : track;
    setTrackState(current => ({ ...current, groups: current.groups.map(group => ({ ...group, tracks: group.tracks.map(updateTrack) })) }));
    setItems((current) => current.map(updateTrack));
    setTagEditorTrack((current) => current ? updateTrack(current) : null);
  };

  const [checkingLibrary, setCheckingLibrary] = useState(false);
  const [checkFeedback, setCheckFeedback] = useState<{scope: string; kind: "updated" | "unchanged" | "failed" | "other-source-unavailable"} | null>(null);
  const libraryCheckInFlight = useRef(false);
  const browserMounted = useRef(true);
  useEffect(() => { browserMounted.current = true; return () => { browserMounted.current = false; }; }, []);

  const sourceOptions = useMemo(() => localLibrarySourceOptions(albums), [albums]);
  const libraryPresentation = localLibrarySelectorPresentation(configuredLocalFolders);
  const librarySelectOptions = useMemo(() => [
    { value: "", label: t("localBrowser.allLibraries") },
    ...sourceOptions.map((source) => ({ value: source.id, label: source.label })),
  ], [sourceOptions, t]);
  const localAlbumSortOptions = useMemo(() => [
    { value: "album-asc", label: t("localBrowser.sortAlbumAsc") },
    { value: "album-desc", label: t("localBrowser.sortAlbumDesc") },
    { value: "artist-asc", label: t("localBrowser.sortArtistAsc") },
    { value: "artist-desc", label: t("localBrowser.sortArtistDesc") },
    { value: "year-newest", label: t("localBrowser.sortYearNewest") },
    { value: "year-oldest", label: t("localBrowser.sortYearOldest") },
    { value: "added-newest", label: t("localBrowser.sortAddedNewest") },
    { value: "added-oldest", label: t("localBrowser.sortAddedOldest") },
  ], [t]);
  const localArtistSortOptions = useMemo(() => [
    { value: "artist-asc", label: t("localBrowser.sortArtistAsc") },
    { value: "artist-desc", label: t("localBrowser.sortArtistDesc") },
  ], [t]);
  const activeSourceId = validLocalLibrarySourceSelection(selectedSourceId, sourceOptions);
  // No source ID uses the same all-folder check as Settings. A bound ID limits discovery.
  const canSelectLibraryCheck = configuredLocalFolders.length > 0;
  const libraryCheckScope = JSON.stringify([configuredLocalFolders, activeSourceId]);
  const checkDisabled = checkingLibrary || !catalog.loaded || catalog.loading || catalog.saveBlocked
    || catalog.mpdBusy || !!catalog.error || isLibraryUpdating || !isPlaybackAvailable
    || !!(tagEditorAlbum || tagEditorTrack || doctorAlbum || fileInfoTrack);
  const handleLibraryCheck = async () => {
    if (!canSelectLibraryCheck || checkDisabled || libraryCheckInFlight.current) return;
    libraryCheckInFlight.current = true;
    const scope = libraryCheckScope;
    const source = activeSourceId ?? undefined;
    setCheckingLibrary(true);
    setCheckFeedback(null);
    try {
      const changed = await refreshCatalogAfterSuccessfulCheck(
        () => audioService.checkLocalLibrary(source, configuredLocalFolders), () => localAlbumCatalog.requestRefresh(),
      );
      if (browserMounted.current) setCheckFeedback({scope, kind: changed ? "updated" : "unchanged"});
    } catch (error) {
      logError("library.check-changes", error);
      if (browserMounted.current) setCheckFeedback({scope, kind:
        error === "LOCAL_INCREMENTAL_CHECK_OTHER_SOURCE_UNAVAILABLE" ? "other-source-unavailable" : "failed"});
    } finally {
      libraryCheckInFlight.current = false;
      if (browserMounted.current) setCheckingLibrary(false);
    }
  };
  // A successful catalog publication advances data and revision together. This
  // switches the render in one snapshot, before async partial/cache cleanup.
  const confirmedPreviewRevision = localAlbumCatalog.getConfirmedRevision();
  const visibleImportPreview = useMemo(
    () => filterImportPreview(importPreviewForCatalog(importPreview, importPreviewRevision, confirmedPreviewRevision), activeSourceId, albumSearch),
    [importPreview, activeSourceId, albumSearch, importPreviewRevision, confirmedPreviewRevision],
  );
  const scopedAlbums = useMemo(
    () => filterLocalAlbumsByScope(albums, activeSourceId, ""),
    [albums, activeSourceId],
  );
  const genreFamilyLabels = useMemo(() => ({
    brazilian: t("localGenreFamilies.brazilian"),
    classical: t("localGenreFamilies.classical"),
    metal: t("localGenreFamilies.metal"),
    electronic: t("localGenreFamilies.electronic"),
    jazz: t("localGenreFamilies.jazz"),
    blues: t("localGenreFamilies.blues"),
    reggae: t("localGenreFamilies.reggae"),
    hipHop: t("localGenreFamilies.hipHop"),
    funkSoul: t("localGenreFamilies.funkSoul"),
    folkCountry: t("localGenreFamilies.folkCountry"),
    rock: t("localGenreFamilies.rock"),
    pop: t("localGenreFamilies.pop"),
    latin: t("localGenreFamilies.latin"),
    world: t("localGenreFamilies.world"),
    soundtrack: t("localGenreFamilies.soundtrack"),
    other: t("localGenreFamilies.other"),
  }), [t]);
  const genreOptions = useMemo(() => deriveLocalGenreFamilies(scopedAlbums,
    family => genreFamilyLabels[family]), [scopedAlbums, genreFamilyLabels]);
  const activeGenre = viewMode === "genres" ? validAlbumGenre(selectedGenre, genreOptions) : null;
  const selectedGenreSummary = genreOptions.find(genre => genre.value === activeGenre);
  const genreAlbums = useMemo(() => activeGenre === null ? [] : sortLocalAlbums(
    filterLocalAlbumsByFamily(filterLocalAlbumsByScope(scopedAlbums, null, albumSearch), activeGenre), albumSortMode),
    [scopedAlbums, albumSearch, activeGenre, albumSortMode]);
  useEffect(() => { if (selectedGenre !== activeGenre) setSelectedGenre(activeGenre); }, [selectedGenre, activeGenre]);
  const filteredAlbums = useMemo(
    () => filterLocalAlbumsByScope(scopedAlbums, null, albumSearch),
    [scopedAlbums, albumSearch],
  );
  const sortedAlbums = useMemo(
    () => albumSortMode === "album-asc" && activeSourceId === null && albumSearch === ""
      ? openingAlbums
      : sortLocalAlbums(filteredAlbums, albumSortMode),
    [openingAlbums, filteredAlbums, albumSortMode, activeSourceId, albumSearch],
  );
  const albumBucketIndices = useMemo(
    () => localAlbumBucketFirstIndices(sortedAlbums, albumSortMode),
    [sortedAlbums, albumSortMode],
  );
  const artists = useMemo(() => deriveLocalArtists(scopedAlbums), [scopedAlbums]);
  const visibleArtists = useMemo(
    () => filterAndSortLocalArtists(artists, artistSearch, artistSortMode),
    [artists, artistSearch, artistSortMode],
  );
  const artistBucketIndices = useMemo(
    () => localArtistBucketFirstIndices(visibleArtists),
    [visibleArtists],
  );
  const yearNavigation = useMemo(
    () => deriveLocalYearNavigation(scopedAlbums),
    [scopedAlbums],
  );
  const artistAlbums = useMemo(
    () => selectedArtist
      ? sortLocalAlbums(albumsForLocalArtist(scopedAlbums, selectedArtist), "album-asc")
      : [],
    [scopedAlbums, selectedArtist],
  );
  const yearAlbums = useMemo(() => {
    const selected = unknownYearSelected
      ? filterLocalAlbumsWithoutYear(scopedAlbums)
      : selectedYear !== null
        ? filterLocalAlbumsByYear(scopedAlbums, selectedYear)
        : [];
    return sortLocalAlbums(selected, "album-asc");
  }, [scopedAlbums, selectedYear, unknownYearSelected]);
  const selectedDecadeSummary = selectedDecade === null
    ? null
    : yearNavigation.decades.find((item) => item.decade === selectedDecade) ?? null;

  useEffect(() => {
    if (selectedSourceId !== activeSourceId) setSelectedSourceId(activeSourceId);
  }, [activeSourceId, selectedSourceId]);

  useEffect(() => {
    if (!selectedArtist || !catalog.loaded || catalog.loading) return;
    if (artistAlbums.length === 0) {
      setSelectedArtist(null);
      onClearInitialArtist?.();
    }
  }, [artistAlbums.length, catalog.loaded, catalog.loading, onClearInitialArtist, selectedArtist]);

  useEffect(() => {
    const valid = validLocalYearSelection(
      yearNavigation,
      selectedDecade,
      selectedYear,
      unknownYearSelected,
    );
    if (valid.decade !== selectedDecade) setSelectedDecade(valid.decade);
    if (valid.year !== selectedYear) setSelectedYear(valid.year);
    if (valid.unknown !== unknownYearSelected) setUnknownYearSelected(valid.unknown);
  }, [selectedDecade, selectedYear, unknownYearSelected, yearNavigation]);

  const showContextBack = Boolean(
    playerTarget
    || initialAlbumId
    || selectedAlbum
    || selectedArtist
    || selectedDecade !== null
    || selectedYear !== null
    || unknownYearSelected
    || activeGenre !== null,
  );
  const headerTitle = selectedAlbum?.title
    ?? selectedArtist
    ?? selectedGenreSummary?.label
    ?? (unknownYearSelected ? t("localBrowser.unknownYear") : null)
    ?? (selectedYear !== null ? String(selectedYear) : null)
    ?? (selectedDecade !== null ? `${selectedDecade}s` : null)
    ?? (viewMode === "albums"
      ? t("localBrowser.albumsTitle")
      : viewMode === "artists"
        ? t("localBrowser.artistsTitle")
        : viewMode === "years"
          ? t("localBrowser.yearsTitle")
          : viewMode === "genres"
            ? t("common.genresLabel")
            : t("localBrowser.foldersTitle"));
  const tabCounts = {
    albums: scopedAlbums.length, artists: artists.length, genres: genreOptions.length,
    years: yearNavigation.decades.reduce((n, decade) => n + decade.years.length, 0) + Number(yearNavigation.unknownCount > 0),
    folders: currentPath === "" ? sourceOptions.length : null,
  };
  const headerCount = selectedAlbum
    ? selectedAlbum.artist
    : selectedArtist
      ? t("localBrowser.albumsCount", { count: artistAlbums.length })
      : activeGenre !== null
        ? t("localBrowser.albumsCount", { count: genreAlbums.length })
      : selectedYear !== null || unknownYearSelected
        ? t("localBrowser.albumsCount", { count: yearAlbums.length })
        : selectedDecadeSummary
          ? t("localBrowser.albumsCount", { count: selectedDecadeSummary.albumCount })
          : viewMode === "albums"
            ? albumSearch.trim() ? t("common.filteredAlbumsCount", { count: filteredAlbums.length, total: scopedAlbums.length }) : t("localBrowser.albumsCount", { count: filteredAlbums.length })
            : viewMode === "artists"
              ? artistSearch.trim() ? t("common.filteredArtistsCount", { count: visibleArtists.length, total: artists.length }) : t("localBrowser.artistsCount", { count: visibleArtists.length })
              : viewMode === "years" || viewMode === "genres"
                ? t("localBrowser.albumsCount", { count: scopedAlbums.length })
                : localLibraryPathLabel(currentPath, sourceOptions) || t("localBrowser.root");

  const handleContextBack = () => {
    if (initialAlbumId && (!selectedAlbum || selectedAlbum.id === initialAlbumId)) {
      if (!tagEditorOpen.current && !doctorAlbum) onAlbumNavigationBack?.();
      return;
    }
    if (playerTarget && ((!selectedAlbum)
      || (playerTarget.view === "album" && selectedAlbum.id === playerTarget.albumId))) {
      onPlayerNavigationBack?.();
      return;
    }
    if (selectedAlbum) {
      setSelectedAlbum(null);
    } else if (selectedArtist) {
      setSelectedArtist(null);
      onClearInitialArtist?.();
    } else if (activeGenre !== null) {
      setSelectedGenre(null);
    } else if (unknownYearSelected) {
      setUnknownYearSelected(false);
    } else if (selectedYear !== null) {
      setSelectedYear(null);
    } else if (selectedDecade !== null) {
      setSelectedDecade(null);
    }
  };

  const selectViewMode = (mode: typeof viewMode) => {
    setViewMode(mode);
    setSelectedGenre(null);
    setSelectedAlbum(null);
    setSelectedArtist(null);
    setSelectedDecade(null);
    setSelectedYear(null);
    setUnknownYearSelected(false);
    onClearInitialArtist?.();
  };

  const selectMetadataAlbum = (albumId: string) => {
    if (tagEditorOpen.current || doctorAlbum || !localAlbumCatalog.getSnapshot().albums.some(album => album.id === albumId)) return;
    setMetadataAlbumId(albumId);
  };
  const openMetadataEditor = (albumId: string) => {
    if (tagEditorOpen.current || doctorAlbum || localAlbumCatalog.getSnapshot() !== catalog || !albumTagEditingAvailable(albumId)) return;
    const album = catalog.albums.find(item => item.id === albumId);
    if (!album) return;
    tagEditorOpen.current = true;
    setDoctorAlbum(null);
    setDoctorReviewContext(null);
    setTagEditorAlbum(album);
  };
  const diagnoseMetadataAlbum = (albumId: string) => {
    if (tagEditorOpen.current || doctorAlbum || localAlbumCatalog.getSnapshot() !== catalog
      || !libraryDoctorAvailable(localAlbumCatalog.getSnapshot(), libraryUpdatingRef.current, albumId)) return;
    const album = catalog.albums.find(item => item.id === albumId);
    if (album) setDoctorAlbum(album);
  };

  return (
    <main className="flex-1 flex flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)] select-none">
      {queueActions.notice}{queueActions.menu}{selection.toolbar}
      {detailUnavailable && <div role="status" className="space-y-3 p-6 text-sm text-[color:var(--theme-fg-secondary,#BBBBBB)]">
        <p>{t("explore.detailUnavailable")}</p>
        <button type="button" disabled={Boolean(tagEditorAlbum || tagEditorTrack || doctorAlbum || fileInfoTrack)}
          onClick={() => onAlbumNavigationBack?.()} className="rounded px-2 py-1 focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">{t("explore.back")}</button>
      </div>}
      <div className={metadataMode && !detailUnavailable ? "contents" : "hidden"}>
        <LocalMetadataView catalog={catalog} updating={isLibraryUpdating} selectedAlbumId={metadataAlbumId}
          dialogsOpen={Boolean(tagEditorAlbum || tagEditorTrack || doctorAlbum || fileInfoTrack)} onSelectAlbum={selectMetadataAlbum}
          onDiagnose={diagnoseMetadataAlbum} onReview={openMetadataEditor}
          onBack={() => { if (!tagEditorOpen.current && !doctorAlbum) onSubviewChange?.("browser"); }} />
      </div>
      <div className={metadataMode ? "hidden" : "contents"}>
      <div className="flex flex-wrap items-center justify-between gap-4 px-8 pt-8 pb-4">
        <div className="flex items-center space-x-3">
          {showContextBack && !(externalBackNavigation && (initialAlbumId && (!selectedAlbum || selectedAlbum.id === initialAlbumId) || initialArtist && !selectedAlbum)) && (
            <button
              onClick={handleContextBack}
              className="p-1.5 rounded-lg bg-[var(--theme-surface-raised,#1E1E1E)] border border-[color:var(--theme-border-soft,#333333)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer mr-1"
              title={t("localBrowser.back")}
            >
              <ArrowLeft size={16} />
            </button>
          )}

          <div>
            <h2 className="text-2xl font-bold text-[color:var(--theme-fg-primary,#ffffff)] tracking-tight">{headerTitle}</h2>
            <p className={selectedAlbum || viewMode === "folders"
              ? "mt-0.5 text-xs text-[color:var(--theme-fg-muted,#888888)]"
              : "mt-1 text-sm font-medium tabular-nums text-[color:var(--theme-fg-secondary,#BBBBBB)]"}>{headerCount}</p>
            {viewMode === "albums" && artworkActivity && (
              <div className="text-[11px] text-[color:var(--theme-fg-muted,#777777)] mt-1 flex flex-wrap items-center gap-x-2">
                <span className="text-[color:var(--theme-fg-accent,#E5A00D)] flex items-center gap-1 max-w-72 truncate" title={artworkActivity.title}>
                  {artworkActivity.kind === "searching" && <Disc3 size={11} className="animate-spin" />}
                  {t(artworkActivity.kind === "searching" ? "artwork.searchingOnline" : "artwork.foundOnline", { title: artworkActivity.title })}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>

      {!selectedAlbum && !selectedArtist && (
        <div className="flex flex-wrap gap-1 border-b border-[color:var(--theme-border-soft,#222222)] px-8 pb-3">
          {([
            ["albums", Grid, t("localBrowser.tabAlbums")],
            ["artists", Users, t("localBrowser.tabArtists")],
            ["years", CalendarDays, t("localBrowser.tabYears")],
            ["genres", Tags, t("common.genresLabel")],
            ["folders", ListTree, t("localBrowser.tabFolders")],
          ] as const).map(([mode, Icon, label]) => (
            <button
              key={mode}
              type="button"
              onClick={() => selectViewMode(mode)}
              className={`flex items-center space-x-1.5 rounded-md px-3 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                viewMode === mode ? "bg-[#E5A00D] text-black shadow" : "text-[color:var(--theme-fg-secondary,#999999)] hover:bg-[var(--theme-surface-raised,#1E1E1E)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
              }`}
            >
              <Icon size={14} />
              <span>{label} {catalog.loaded && !catalog.error && tabCounts[mode] !== null && <span className="tabular-nums opacity-75">({tabCounts[mode]})</span>}</span>
            </button>
          ))}
        </div>
      )}

      {!selectedAlbum && viewMode !== "folders" && (
        <div className="flex flex-wrap items-center gap-3 border-b border-[color:var(--theme-border-soft,#222222)] px-8 py-3">
          {libraryPresentation.mode !== "empty" && <div className="flex items-center gap-2 text-xs text-[color:var(--theme-fg-muted,#888888)]">
            <span>{t("localBrowser.libraryScope")}</span>
            {libraryPresentation.mode === "single" ? (
              <span className="max-w-44 truncate text-[color:var(--theme-fg-primary,#ffffff)]" title={libraryPresentation.label}>{libraryPresentation.label}</span>
            ) : <ToolbarSelect
              value={activeSourceId ?? ""}
              options={librarySelectOptions}
              onChange={(nextValue) => setSelectedSourceId(nextValue || null)}
              label={t("localBrowser.libraryScope")}
              className="max-w-44"
            />}
          </div>}
          {(viewMode === "albums" || (viewMode === "genres" && activeGenre !== null)) && (
            <>
              <div className="relative flex w-64 max-w-full items-center">
                <Search size={14} className="absolute left-3 text-[color:var(--theme-fg-muted,#666666)]" />
                <input type="text" value={albumSearch} onChange={(event) => setAlbumSearch(event.target.value)} placeholder={t("localBrowser.filterPlaceholder")} className="w-full rounded-lg border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#1A1A1A)] py-1.5 pl-9 pr-3 text-xs text-[color:var(--theme-fg-primary,#ffffff)] placeholder-[color:var(--theme-fg-muted,#666666)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)]" />
              </div>
              <div className="flex items-center gap-2 text-xs text-[color:var(--theme-fg-muted,#888888)]">
                <span>{t("localBrowser.sortLabel")}</span>
                <ToolbarSelect
                  value={albumSortMode}
                  options={localAlbumSortOptions}
                  onChange={(nextValue) => setAlbumSortMode(nextValue as LocalAlbumSortMode)}
                  label={t("localBrowser.sortLabel")}
                  className="max-w-44"
                />
              </div>
            </>
          )}
          {viewMode === "artists" && !selectedArtist && (
            <>
              <div className="relative flex w-64 max-w-full items-center">
                <Search size={14} className="absolute left-3 text-[color:var(--theme-fg-muted,#666666)]" />
                <input type="text" value={artistSearch} onChange={(event) => setArtistSearch(event.target.value)} placeholder={t("localBrowser.artistSearchPlaceholder")} className="w-full rounded-lg border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#1A1A1A)] py-1.5 pl-9 pr-3 text-xs text-[color:var(--theme-fg-primary,#ffffff)] placeholder-[color:var(--theme-fg-muted,#666666)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)]" />
              </div>
              <div className="flex items-center gap-2 text-xs text-[color:var(--theme-fg-muted,#888888)]">
                <span>{t("localBrowser.sortLabel")}</span>
                <ToolbarSelect
                  value={artistSortMode}
                  options={localArtistSortOptions}
                  onChange={(nextValue) => setArtistSortMode(nextValue as LocalArtistSortMode)}
                  label={t("localBrowser.sortLabel")}
                  className="max-w-44"
                />
              </div>
            </>
          )}
          {(viewMode === 'albums' || !!selectedArtist || activeGenre !== null || selectedYear !== null || unknownYearSelected) && <AlbumViewControls value={albumPresentation} onChange={changeAlbumPresentation} />}
          {canSelectLibraryCheck && <div className="ml-auto flex min-w-0 shrink-0 items-center gap-2">
            {checkFeedback?.scope === libraryCheckScope && <span role="status"
              title={checkFeedback.kind === "other-source-unavailable" ? t("localBrowser.checkLibrary.otherSourceUnavailableHelp") : undefined}
              className="max-w-40 truncate text-[11px] text-[color:var(--theme-fg-muted,#888888)]">
              {t(checkFeedback.kind === "updated" ? "localBrowser.checkLibrary.updated"
                : checkFeedback.kind === "unchanged" ? "localBrowser.checkLibrary.unchanged"
                : checkFeedback.kind === "other-source-unavailable" ? "localBrowser.checkLibrary.otherSourceUnavailable" : "localBrowser.checkLibrary.failed")}
            </span>}
            <button type="button" onClick={handleLibraryCheck} disabled={checkDisabled} aria-busy={checkingLibrary}
              title={t(checkingLibrary ? "localBrowser.checkLibrary.checking" : activeSourceId !== null || libraryPresentation.mode === "single"
                ? "localBrowser.checkLibrary.help" : "localBrowser.checkLibrary.helpAll")}
              className="inline-flex shrink-0 items-center gap-1.5 rounded px-2 py-1 text-xs text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-default disabled:opacity-50">
              <RefreshCw size={12} aria-hidden="true" className={checkingLibrary ? "animate-spin motion-reduce:animate-none" : ""} />
              {t("localBrowser.checkLibrary.button")}
            </button>
          </div>}
        </div>
      )}

      {!selectedAlbum && !selectedArtist && viewMode === "albums" && isTextLocalAlbumSort(albumSortMode) && (
        <nav
          aria-label={t("localBrowser.albumIndex")}
          className="flex flex-wrap items-center gap-1 border-b border-[color:var(--theme-border-soft,#222222)] px-8 py-2"
        >
          {LOCAL_ALBUM_INDEX_BUCKETS.map((bucket) => {
            const index = albumBucketIndices.get(bucket);
            const available = index !== undefined;
            const label = t("localBrowser.jumpToBucket", { bucket });
            return (
              <button
                key={bucket}
                type="button"
                disabled={!available}
                title={label}
                aria-label={label}
                onClick={() => {
                  if (index !== undefined) albumGridRef.current?.scrollToIndex(index);
                }}
                className="h-6 min-w-6 rounded px-1 text-[11px] font-semibold text-[color:var(--theme-fg-secondary,#999999)] transition-colors hover:bg-[var(--theme-surface-hover,#282828)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-default disabled:text-[color:var(--theme-fg-faint,#444444)] disabled:hover:bg-transparent"
              >
                {bucket}
              </button>
            );
          })}
        </nav>
      )}
      {!selectedAlbum && !selectedArtist && viewMode === "artists" && (
        <nav aria-label={t("localBrowser.artistIndex")} className="flex flex-wrap items-center gap-1 border-b border-[color:var(--theme-border-soft,#222222)] px-8 py-2">
          {LOCAL_ALBUM_INDEX_BUCKETS.map((bucket) => {
            const index = artistBucketIndices.get(bucket);
            const label = t("localBrowser.jumpToArtistBucket", { bucket });
            return (
              <button key={bucket} type="button" disabled={index === undefined} title={label} aria-label={label} onClick={() => index !== undefined && artistListRef.current?.scrollToIndex(index)} className="h-6 min-w-6 rounded px-1 text-[11px] font-semibold text-[color:var(--theme-fg-secondary,#999999)] transition-colors hover:bg-[var(--theme-surface-hover,#282828)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-default disabled:text-[color:var(--theme-fg-faint,#444444)] disabled:hover:bg-transparent">
                {bucket}
              </button>
            );
          })}
        </nav>
      )}

      <div aria-hidden={!catalogActivity} className="h-9 min-w-0 shrink-0 px-8 text-xs text-[color:var(--theme-fg-accent,#E5A00D)] flex items-center gap-2 border-b border-[color:var(--theme-border-soft,#222222)]">
        {catalogActivity && (
          <>
            <Disc3 aria-hidden="true" size={14} className="animate-spin shrink-0" />
            <span className="min-w-0 truncate" title={t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}>
              {t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </span>
          </>
        )}
      </div>
      {playbackFailure && (
        <div role="alert" className="px-8 py-2 text-xs text-[color:var(--theme-fg-error,var(--color-red-400))] border-b border-[color:var(--theme-border-soft,#222222)]">
          {t(`player.requestError.${playbackFailure}`)}
        </div>
      )}
      {catalog.error && (
        <LocalCatalogReadError catalog={catalog} updating={isLibraryUpdating} available={isPlaybackAvailable} onRetry={() => {
          if (canRetryLocalCatalog(localAlbumCatalog.getSnapshot(), libraryUpdatingRef.current, isPlaybackAvailable)) localAlbumCatalog.requestRefresh();
        }} />
      )}
      <div ref={setScrollContainer} className="flex-1 overflow-y-auto p-8">

        {selectedAlbum ? (
          <div className="space-y-8 animate-in fade-in duration-100">
            <div className="flex items-end space-x-6">
              <div className="w-52 h-52 rounded-xl bg-[var(--theme-surface-raised,#202020)] border border-[color:var(--theme-border-soft,#2B2B2B)] overflow-hidden shrink-0 shadow-2xl flex items-center justify-center">
                {albumCover ? (
                  <img src={albumCover} alt={selectedAlbum.title} className="w-full h-full object-cover" />
                ) : (
                  <AlbumArtworkPlaceholder size="detail" />
                )}
              </div>

              <div className="space-y-3">
                <span className="text-xs font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider flex items-center space-x-1">
                  <Sparkles size={13} />
                  <span>{t("localBrowser.albumsTitle")}</span>
                </span>
                <h1 className="text-3xl font-black text-[color:var(--theme-fg-primary,#ffffff)]">{selectedAlbum.title}</h1>
                <button
                  type="button"
                  onClick={() => {
                    setSelectedArtist(selectedAlbum.artist);
                    setSelectedAlbum(null);
                    setViewMode("artists");
                    setSelectedGenre(null);
                    setSelectedDecade(null);
                    setSelectedYear(null);
                    setUnknownYearSelected(false);
                  }}
                  className="text-base text-[color:var(--theme-fg-secondary,#CCCCCC)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] font-medium transition-colors cursor-pointer text-left block"
                  title={t("localBrowser.viewDiscography")}
                >
                  {selectedAlbum.artist}
                </button>
                <p className="text-xs text-[color:var(--theme-fg-muted,#777777)]">
                  {selectedAlbum.year ? `${selectedAlbum.year} • ` : ""}
                  {t("localBrowser.trackCount", { count: tracksReady ? albumTracks.length : selectedAlbum.track_count })}
                </p>
                {albumLocations?.album === selectedAlbum && albumLocations.paths.length > 0 && (
                  <div className="space-y-1 text-[11px] text-[color:var(--theme-fg-muted,#777777)] min-w-0">
                    <p className="font-semibold">{t("localBrowser.location")}</p>
                    {albumLocations.paths.map((location, index) => (
                      <div key={`${location.absolutePath}-${index}`} className="min-w-0">
                        {location.label && <span className="block text-[color:var(--theme-fg-secondary,#999999)]">{location.label}</span>}
                        <span
                          className="block w-full max-w-xl truncate select-text cursor-text"
                          title={location.absolutePath}
                        >
                          {location.absolutePath}
                        </span>
                      </div>
                    ))}
                  </div>
                )}

                <div className="pt-2 flex flex-wrap items-center gap-3">
                  <button
                    onClick={() => handlePlayEntireAlbum(selectedAlbum, albumTracks.length ? albumTracks : undefined, 0)}
                    disabled={!isPlaybackAvailable || !tracksReady}
                    className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    <Play size={16} fill="black" />
                    <span>{t("localBrowser.playAlbum")}</span>
                  </button>

                  <button
                    type="button"
                    onClick={() =>
                      setPlaylistItems(
                        albumTracks.map((track) => toPlaylistItem(track, selectedAlbum)),
                      )
                    }
                    disabled={albumTracks.length === 0}
                    className="flex items-center space-x-2 rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-raised,#1E1E1E)] px-4 py-2.5 text-xs font-bold text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-hover,#282828)] disabled:opacity-50"
                    title={t("playlists.addAlbum")}
                  >
                    <ListPlus size={16} />
                    <span>{t("playlists.addAlbum")}</span>
                  </button>

                  <button
                    onClick={(e) => handleToggleFavoriteLocal(e, selectedAlbum, albumCover)}
                    className="p-2.5 rounded-xl bg-[var(--theme-surface-raised,#1E1E1E)] border border-[color:var(--theme-border-soft,#2B2B2B)] hover:bg-[var(--theme-surface-hover,#282828)] text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer"
                    title={favoriteIds.has(selectedAlbum.folder_path) ? t("favorites.removeFavorite") : t("favorites.title")}
                  >
                    <Heart
                      size={16}
                      className={favoriteIds.has(selectedAlbum.folder_path) ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#888888)]"}
                      fill={favoriteIds.has(selectedAlbum.folder_path) ? "#E5A00D" : "none"}
                    />
                  </button>
                  <button
                    type="button"
                    disabled={Boolean(tagEditorAlbum || tagEditorTrack || doctorAlbum || fileInfoTrack)}
                    onClick={() => {
                      if (!tagEditorOpen.current && !doctorAlbum) {
                        setMetadataAlbumId(selectedAlbum.id);
                        onSubviewChange?.("metadata");
                      }
                    }}
                    className="flex items-center gap-1.5 rounded px-1 py-1 text-xs text-[color:var(--theme-fg-secondary,#999999)] transition-colors hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-not-allowed disabled:opacity-40"
                    title={t("localMetadata.review")}
                  >
                    <Pencil size={15} />
                    <span>{t("localMetadata.review")}</span>
                  </button>
                </div>
              </div>
            </div>

            <div className="bg-[var(--theme-surface-base,#141414)] border border-[color:var(--theme-border-soft,#222222)] rounded-xl overflow-hidden divide-y divide-[color:var(--theme-border-soft,#1D1D1D)]">
              {!tracksReady && (
                <div role={catalogConfirmed && trackState.albumId === selectedAlbum.id && trackState.phase === "error" ? "alert" : "status"} className="px-4 py-4 text-sm text-[color:var(--theme-fg-secondary,#CCCCCC)]">
                  {catalogConfirmed && trackState.albumId === selectedAlbum.id && trackState.phase === "error" ? trackState.error : t(catalogConfirmed && trackState.phase === "loading" ? "localBrowser.loadingTracks" : "localBrowser.waitingTracks")}
                </div>
              )}
              <div className="grid grid-cols-[2rem_minmax(0,1fr)_auto] px-4 py-2.5 text-[11px] font-bold text-[color:var(--theme-fg-muted,#666666)] uppercase tracking-wider bg-[var(--theme-surface-panel,#181818)]">
                <span className="text-center">#</span>
                <span className="min-w-0">{t("plex.tracks")}</span>
                <span className="text-right flex items-center justify-end space-x-1">
                  <Clock size={12} />
                  <span>{t("localBrowser.duration")}</span>
                </span>
              </div>

              {albumSections.map((section) => (
                <React.Fragment key={section.disc.folder_path}>
                  {selectedAlbum.discs.length > 1 && (
                    <div className="px-4 py-2.5 text-xs font-bold text-[color:var(--theme-fg-accent,#E5A00D)] bg-[var(--theme-surface-panel,#181818)]">
                      {section.disc.label}
                    </div>
                  )}
                  {section.tracks.map((track, idx) => (
                    <div
                      key={track.path}
                      onClick={event => { if (!selection.click(event, section.startIndex + idx)) void handlePlayEntireAlbum(selectedAlbum, albumTracks, section.startIndex + idx); }}
                      onContextMenu={event => queueActions.open(event, queueRequest(track, selectedAlbum))}
                  aria-disabled={!isPlaybackAvailable}
                  className={`${selection.checked(section.startIndex + idx) ? "track-selected " : ""}grid grid-cols-[2rem_minmax(0,1fr)_auto] px-4 py-3 text-xs items-center transition-colors group ${
                    isPlaybackAvailable
                      ? "hover:bg-[var(--theme-surface-raised,#1E1E1E)] cursor-pointer"
                      : "opacity-60 cursor-not-allowed"
                  }`}
                >
                  <span className="text-center font-mono text-[color:var(--theme-fg-muted,#666666)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)]">
                    {selection.checkbox(section.startIndex + idx)}<span className="ml-1">{idx + 1}</span>
                  </span>
                  <div className="min-w-0 flex flex-col pr-2">
                    <span className="font-semibold text-[color:var(--theme-fg-primary,#ffffff)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate">
                      {track.title || track.name}
                    </span>
                    <span className="text-[11px] text-[color:var(--theme-fg-muted,#777777)] truncate">
                      {track.artist || selectedAlbum.artist}
                    </span>
                  </div>
                  <div className="flex items-center justify-end gap-1.5">
                    <span className="font-mono text-[color:var(--theme-fg-muted,#888888)]">
                      {formatDuration(track.duration)}
                    </span>
                    <button
                      type="button"
                      onClick={(event) => {
                        event.stopPropagation();
                        setPlaylistItems([toPlaylistItem(track, selectedAlbum)]);
                      }}
                      className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                      title={t("playlists.addTrack")}
                    >
                      <ListPlus size={15} />
                    </button>
                    {queueActions.trigger(queueRequest(track, selectedAlbum))}
                    <button type="button" disabled={!catalogInspectionAllowed} onClick={(event) => { event.stopPropagation(); setFileInfoTrack({ track: track, catalog: localAlbumCatalog.getSnapshot() }); }}
                      title={t("fileInfo.title")} aria-label={t("fileInfo.forTrack", { title: track.title || track.name })}
                      className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]">
                      <Info size={14} />
                    </button>
                    <button
                      type="button"
                      onClick={(event) => {
                        event.stopPropagation();
                        if (catalogInspectionAllowed) setTagEditorTrack(track);
                      }}
                      className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                      title={t("tagEditor.edit")}
                      aria-label={t("tagEditor.editTrack", { title: track.title || track.name })}
                    >
                      <Pencil size={14} />
                    </button>
                  </div>
                    </div>
                  ))}
                </React.Fragment>
              ))}
            </div>
          </div>
        ) : selectedArtist ? (
          <div className="space-y-6 animate-in fade-in duration-100">
            {artistAlbums.length === 0 && emptyCatalogState === "indexing" ? (
              <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
                {t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
              </div>
            ) : emptyCatalogState === "error" ? null : artistAlbums.length === 0 ? (
              <div className="h-60 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
                <Disc3 size={40} className="opacity-40" />
                <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
              </div>
            ) : (
              <AlbumCatalog presentation={albumPresentation}
                items={artistAlbums}
                getItemKey={(album) => album.id}
                scrollContainer={scrollContainer}
                resetKey={JSON.stringify([activeSourceId, selectedArtist])}
                renderItem={(album) => (
                  <LocalAlbumCard layout={albumPresentation.layout}
                    readEnabled={!metadataMode && catalog.loaded}
                    album={album}
                    isFavorite={favoriteIds.has(album.folder_path)}
                    onToggleFavorite={handleToggleFavoriteLocal}
                    onClick={() => handleSelectAlbum(album)}
                    onPlayQuick={(e) => {
                      e.stopPropagation();
                      handlePlayEntireAlbum(album);
                    }}
                    isPlaybackAvailable={isPlaybackAvailable}
                    onlineArtworkEnabled={onlineArtworkEnabled && !catalog.manualCheckPlayback}
                    isLibraryUpdating={artworkBlocked}
                  />
                )}
              />
            )}
          </div>
        ) : viewMode === "albums" ? (
          <>
            {filteredAlbums.length === 0 && visibleImportPreview.length > 0 ? null : emptyCatalogState === "indexing" ? (
              <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
                {t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
              </div>
            ) : emptyCatalogState === "error" ? null
            : filteredAlbums.length === 0 ? (
              <div className="h-60 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
                <Disc3 size={40} className="opacity-40" />
                <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
              </div>
            ) : (
              <AlbumCatalog presentation={albumPresentation}
                ref={albumGridRef}
                items={sortedAlbums}
                getItemKey={(album) => album.id}
                scrollContainer={scrollContainer}
                resetKey={JSON.stringify([localAlbumNavigationResetKey(activeSourceId, albumSearch, albumSortMode)])}
                renderItem={(album) => (
                  <LocalAlbumCard layout={albumPresentation.layout}
                    readEnabled={!metadataMode && catalog.loaded}
                    album={album}
                    isFavorite={favoriteIds.has(album.folder_path)}
                    onToggleFavorite={handleToggleFavoriteLocal}
                    onClick={() => handleSelectAlbum(album)}
                    onPlayQuick={(e) => {
                      e.stopPropagation();
                      handlePlayEntireAlbum(album);
                    }}
                    isPlaybackAvailable={isPlaybackAvailable && catalogConfirmed}
                    onlineArtworkEnabled={onlineArtworkEnabled && !catalog.manualCheckPlayback}
                    isLibraryUpdating={artworkBlocked}
                  />
                )}
              />
            )}
            <LocalImportPreviewSection albums={visibleImportPreview} scrollContainer={scrollContainer} t={t} artworkCache={importArtworkCache} />
          </>
        ) : viewMode === "artists" ? (
          emptyCatalogState === "indexing" ? (
            <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
              {t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </div>
          ) : emptyCatalogState === "error" ? null
          : visibleArtists.length === 0 ? (
            <div className="h-60 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
              <Users size={40} className="opacity-40" />
              <span className="text-xs">{t("localBrowser.emptyArtists")}</span>
            </div>
          ) : (
            <VirtualList
              ref={artistListRef}
              items={visibleArtists}
              rowHeight={56}
              getItemKey={(artist) => artist.id}
              scrollContainer={scrollContainer}
              resetKey={JSON.stringify([activeSourceId, artistSearch, artistSortMode])}
              renderItem={(artist) => (
                <button
                  type="button"
                  onClick={() => setSelectedArtist(artist.name)}
                  className="flex h-full w-full items-center justify-between border-b border-[color:var(--theme-border-soft,#1D1D1D)] px-4 text-left transition-colors hover:bg-[var(--theme-surface-raised,#1E1E1E)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
                >
                  <span className="truncate pr-4 text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)]">{artist.name}</span>
                  <span className="shrink-0 text-xs text-[color:var(--theme-fg-muted,#777777)]">{t("localBrowser.albumsCount", { count: artist.albumCount })}</span>
                </button>
              )}
            />
          )
        ) : viewMode === "genres" ? (
          activeGenre !== null ? (
            genreAlbums.length === 0 ? (
              <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
                {t("localBrowser.emptyAlbums")}
              </div>
            ) : (
              <AlbumCatalog presentation={albumPresentation}
                items={genreAlbums}
                getItemKey={(album) => album.id}
                scrollContainer={scrollContainer}
                resetKey={JSON.stringify([activeSourceId, "genre", activeGenre, albumSearch, albumSortMode])}
                renderItem={(album) => (
                  <LocalAlbumCard layout={albumPresentation.layout}
                    readEnabled={!metadataMode && catalog.loaded}
                    album={album}
                    isFavorite={favoriteIds.has(album.folder_path)}
                    onToggleFavorite={handleToggleFavoriteLocal}
                    onClick={() => handleSelectAlbum(album)}
                    onPlayQuick={(event) => {
                      event.stopPropagation();
                      handlePlayEntireAlbum(album);
                    }}
                    isPlaybackAvailable={isPlaybackAvailable}
                    onlineArtworkEnabled={onlineArtworkEnabled && !catalog.manualCheckPlayback}
                    isLibraryUpdating={artworkBlocked}
                  />
                )}
              />
            )
          ) : emptyCatalogState === "indexing" ? (
            <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
              {t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </div>
          ) : emptyCatalogState === "error" ? null : (
            <AlbumGenreGrid genres={genreOptions} onSelect={setSelectedGenre}
              albumCountLabel={count => t("localBrowser.albumsCount", { count })} />
          )
        ) : viewMode === "years" ? (
          selectedYear !== null || unknownYearSelected ? (
            yearAlbums.length === 0 ? (
              <div className="h-60 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
                <Disc3 size={40} className="opacity-40" />
                <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
              </div>
            ) : (
              <AlbumCatalog presentation={albumPresentation}
                items={yearAlbums}
                getItemKey={(album) => album.id}
                scrollContainer={scrollContainer}
                resetKey={JSON.stringify([activeSourceId, selectedYear, unknownYearSelected])}
                renderItem={(album) => (
                  <LocalAlbumCard layout={albumPresentation.layout}
                    readEnabled={!metadataMode && catalog.loaded}
                    album={album}
                    isFavorite={favoriteIds.has(album.folder_path)}
                    onToggleFavorite={handleToggleFavoriteLocal}
                    onClick={() => handleSelectAlbum(album)}
                    onPlayQuick={(event) => {
                      event.stopPropagation();
                      handlePlayEntireAlbum(album);
                    }}
                    isPlaybackAvailable={isPlaybackAvailable}
                    onlineArtworkEnabled={onlineArtworkEnabled && !catalog.manualCheckPlayback}
                    isLibraryUpdating={artworkBlocked}
                  />
                )}
              />
            )
          ) : selectedDecadeSummary ? (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-4">
              {selectedDecadeSummary.years.map((year) => (
                <button
                  key={year.year}
                  type="button"
                  onClick={() => setSelectedYear(year.year)}
                  className="rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#181818)] p-5 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)] hover:bg-[var(--theme-surface-raised,#1E1E1E)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
                >
                  <span className="block text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{year.year}</span>
                  <span className="mt-1 block text-xs text-[color:var(--theme-fg-muted,#777777)]">{t("localBrowser.albumsCount", { count: year.albumCount })}</span>
                </button>
              ))}
            </div>
          ) : emptyCatalogState === "indexing" ? (
            <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
              {t(catalogActivity === "indexing" ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </div>
          ) : emptyCatalogState === "error" ? null : scopedAlbums.length === 0 ? (
            <div className="h-60 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
              <Disc3 size={40} className="opacity-40" />
              <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
            </div>
          ) : (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-4">
              {yearNavigation.decades.map((decade) => (
                <button
                  key={decade.decade}
                  type="button"
                  onClick={() => setSelectedDecade(decade.decade)}
                  aria-label={t("localBrowser.decadeLabel", { decade: decade.decade })}
                  className="rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#181818)] p-5 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)] hover:bg-[var(--theme-surface-raised,#1E1E1E)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
                >
                  <span className="block text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{t("localBrowser.decadeShort", { decade: decade.decade })}</span>
                  <span className="mt-1 block text-xs text-[color:var(--theme-fg-muted,#777777)]">{t("localBrowser.albumsCount", { count: decade.albumCount })}</span>
                </button>
              ))}
              {yearNavigation.unknownCount > 0 && (
                <button
                  type="button"
                  onClick={() => setUnknownYearSelected(true)}
                  className="rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#181818)] p-5 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)] hover:bg-[var(--theme-surface-raised,#1E1E1E)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
                >
                  <span className="block text-lg font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{t("localBrowser.unknownYear")}</span>
                  <span className="mt-1 block text-xs text-[color:var(--theme-fg-muted,#777777)]">{t("localBrowser.albumsCount", { count: yearNavigation.unknownCount })}</span>
                </button>
              )}
            </div>
          )
        ) : (
          <div className="space-y-4">
            {currentPath && (
              <button
                onClick={() => {
                  const parts = currentPath.split("/");
                  parts.pop();
                  setCurrentPath(parts.join("/"));
                }}
                className="flex items-center space-x-2 text-xs font-semibold text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer mb-2"
              >
                <ArrowLeft size={14} />
                <span>{t("localBrowser.upOneLevel")}</span>
              </button>
            )}

            {loadingFolders ? (
              <div className="h-40 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
                {t("localBrowser.loadingFolder")}
              </div>
            ) : items.length === 0 ? (
              <div className="h-40 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
                {t("localBrowser.emptyFolder")}
              </div>
            ) : (
              <div className="divide-y divide-[color:var(--theme-border-soft,#1D1D1D)] bg-[var(--theme-surface-base,#141414)] rounded-xl border border-[color:var(--theme-border-soft,#222222)]">
                {items.map((item) => {
                  const isDir = item.item_type === "directory";
                  return (
                    <div
                      key={item.path}
                      onContextMenu={isDir ? undefined : event => queueActions.open(event, queueRequest(item))}
                      onClick={event => {
                        if (!isDir && selection.click(event, folderSelectionIndices.get(item.path)!)) return;
                        if (isDir) {
                          setCurrentPath(item.path);
                        } else if (isPlaybackAvailable) {
                          const meta = [
                            {
                              title: item.title || item.name,
                              artist: item.artist || "",
                              album: item.album || "",
                              thumb: folderCover,
                              uri: item.path,
			      duration: item.duration,
                            },
                          ];
                          void runLocalPlayback(intent => audioService.playTracks(meta, 0, intent));
                        }
                      }}
                      aria-disabled={!isDir && !isPlaybackAvailable}
                      className={`${!isDir && selection.checked(folderSelectionIndices.get(item.path)!) ? "track-selected " : ""}flex items-center justify-between p-3 transition-colors group ${
                        isDir || isPlaybackAvailable
                          ? "hover:bg-[var(--theme-surface-raised,#1E1E1E)] cursor-pointer"
                          : "opacity-60 cursor-not-allowed"
                      }`}
                    >
                      <div className="flex items-center space-x-3 truncate mr-4">
                        {isDir ? (
                          <Folder size={18} className="text-[color:var(--theme-fg-accent,#E5A00D)] shrink-0" />
                        ) : (
                          selection.checkbox(folderSelectionIndices.get(item.path)!)
                        )}
                        <span className="text-xs font-medium text-[color:var(--theme-fg-primary,#ffffff)] truncate">
                          {isDir && !currentPath ? localLibraryPathLabel(item.path, sourceOptions) : item.title || item.name}
                        </span>
                      </div>

                      {!isDir && (
                        <div className="flex items-center gap-3">
                          <span className="text-xs font-mono text-[color:var(--theme-fg-muted,#666666)]">
                            {formatDuration(item.duration)}
                          </span>
                          <button
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              setPlaylistItems([toPlaylistItem(item)]);
                            }}
                            className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                            title={t("playlists.addTrack")}
                          >
                            <ListPlus size={15} />
                          </button>
                          {queueActions.trigger(queueRequest(item))}
                          <button type="button" disabled={!catalogInspectionAllowed} onClick={(event) => { event.stopPropagation(); setFileInfoTrack({ track: item, catalog: localAlbumCatalog.getSnapshot() }); }}
                            title={t("fileInfo.title")} aria-label={t("fileInfo.forTrack", { title: item.title || item.name })}
                            className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]">
                            <Info size={14} />
                          </button>
                          <button
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              if (catalogInspectionAllowed) setTagEditorTrack(item);
                            }}
                            className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                            title={t("tagEditor.edit")}
                            aria-label={t("tagEditor.editTrack", { title: item.title || item.name })}
                          >
                            <Pencil size={14} />
                          </button>
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </div>
      </div>
      {fileInfoTrack && <LocalFileInfoModal path={fileInfoTrack.track.path}
        displayName={fileInfoTrack.track.title || fileInfoTrack.track.name}
        available={catalogInspectionAllowed && localAlbumCatalog.getSnapshot() === fileInfoTrack.catalog}
        onClose={() => setFileInfoTrack(null)} />}
      {(tagEditorAlbum || tagEditorTrack || doctorAlbum || fileInfoTrack) && <span data-local-tag-dialog-open hidden />}
      {playlistItems && (
        <PlaylistPickerModal
          items={playlistItems}
          title={
            playlistItems.length === 1
              ? t("playlists.addTrack")
              : t("playlists.addAlbum")
          }
          onClose={() => setPlaylistItems(null)}
        />
      )}
      {tagEditorAlbum && (
        <LocalAlbumTagEditorModal
          albumId={tagEditorAlbum.id}
          displayName={tagEditorAlbum.title}
          available={catalogInspectionAllowed && albums.some((album) => album.id === tagEditorAlbum.id)}
          catalogVersion={catalog}
          reviewContext={doctorReviewContext}
          isAvailable={() => localAlbumCatalog.getSnapshot() === catalog && albumTagEditingAvailable(tagEditorAlbum.id)}
          onBatchFinished={(result) => {
            if (result?.tracks.some((track) => track.mpd_refresh.state === "accepted")) {
              localAlbumCatalog.waitForMpd();
            } else {
              localAlbumCatalog.requestRefresh();
            }
          }}
          onClose={() => { setTagEditorAlbum(null); setDoctorReviewContext(null); }}
        />
      )}
      {doctorAlbum && doctorSelectedAlbum.current === doctorAlbum.id && !tagEditorAlbum && !tagEditorTrack && (
        <LibraryDoctorModal
          albumId={doctorAlbum.id}
          displayName={doctorAlbum.title}
          available={catalogInspectionAllowed && albums.some(album => album.id === doctorAlbum.id)}
          catalogVersion={catalog}
          isAvailable={() => localAlbumCatalog.getSnapshot() === catalog
            && doctorSelectedAlbum.current === doctorAlbum.id
            && !tagEditorOpen.current
            && libraryDoctorAvailable(localAlbumCatalog.getSnapshot(), libraryUpdatingRef.current, doctorAlbum.id)}
          onReview={context => {
            if (tagEditorOpen.current || context.albumId !== doctorAlbum.id
              || doctorSelectedAlbum.current !== context.albumId || localAlbumCatalog.getSnapshot() !== catalog
              || !albumTagEditingAvailable(context.albumId)) return false;
            tagEditorOpen.current = true;
            setDoctorAlbum(null);
            setDoctorReviewContext(context);
            setTagEditorAlbum(doctorAlbum);
            return true;
          }}
          onClose={() => setDoctorAlbum(null)}
        />
      )}
      {tagEditorTrack && (
        <LocalTagEditorModal
          path={tagEditorTrack.path}
          displayName={tagEditorTrack.title || tagEditorTrack.name}
          onSaved={(tags) => handleLocalTagsSaved(tagEditorTrack.path, tags)}
          onClose={() => setTagEditorTrack(null)}
        />
      )}
    </main>
  );
};

export default LocalBrowserView;
