import { configuredSignalRoute } from "./utils/signalPath";
import { logError } from "./utils/frontendDiagnostics";
import React, { useEffect, useLayoutEffect, useState, useRef, useCallback, useMemo, useSyncExternalStore, memo } from "react";
import {
  Disc3,
  Folder,
  Server,
  Sparkles,
  Play,
  Settings,
  ArrowLeft,
  Search,
  X,
  User,
  Info,
  RefreshCw,
  CheckCircle2,
  CircleAlert,
  Heart,
  ListMusic,
  ExternalLink,
  ListPlus,
  Compass,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlayerBar } from "./components/PlayerBar";
import { AlbumView } from "./components/AlbumView";
import { ArtistView } from "./components/ArtistView";
import { QueueDrawer } from "./components/QueueDrawer";
import { ExploreView } from "./components/ExploreView";
import type { ExploreContext, ExploreEntry, ExploreSourceState } from "./utils/explore";
import { exploreEntryIsCurrent } from "./utils/explore";
import { PlexArtistPortraitCache } from "./utils/plexArtistPortraits";
import { PlexCatalogCoordinator } from "./utils/plexCatalogCoordinator";
import { LocalBrowserView, localAlbumCatalog, type LocalSubview } from "./components/LocalBrowserView";
import { localCatalogActivity } from "./utils/localAlbumCatalog";
import { localArtworkEnrichment, localArtworkResolver } from "./utils/localArtworkSession";
import { artworkReadBlocked } from "./utils/localArtwork";
import { createMainSectionPreference, type MainSection } from "./utils/appBootstrap";
import { FavoritesView } from "./components/FavoritesView";
import { PlaylistsView } from "./components/PlaylistsView";
import { PlaylistPickerModal } from "./components/PlaylistPickerModal";
import { SettingsModal, QueueClearConfirmationDialog, formatConfigError } from "./components/SettingsModal";
import { AboutModal } from "./components/AboutModal";
import { WelcomeWizard } from "./components/WelcomeWizard";
import { PlexImage } from "./components/PlexImage";
import { VirtualAlbumGrid, type VirtualAlbumGridHandle } from "./components/VirtualAlbumGrid";
import { VirtualList, type VirtualListHandle } from "./components/VirtualList";
import { ToolbarSelect } from "./components/ToolbarSelect";
import { deriveAlbumGenres, filterAlbumsByGenre, validAlbumGenre } from "./utils/albumGenres";
import { AlbumGenreGrid } from "./components/AlbumGenreGrid";
import { SidebarNavItem } from "./components/SidebarNavItem";
import { plexService } from "./services/plex";
import { audioService, LOCAL_LIBRARY_RECONCILED_EVENT } from "./services/audio";
import { configService } from "./services/config";
import { createLocalImportPreview } from "./utils/localImportPreview";
import type { PreviewAlbum } from "./types/localImportPreview";
import { configSavePending, monitorConfigSave } from "./utils/configSaveMonitor";
import type { ConfigSavePhase, QueueClearAuthorization } from "./types/config";
import { createQueueClearConfirmationFlow, queueClearBinding, type QueueClearConfirmation } from "./utils/queueClearConfirmation";
import { favoritesService } from "./services/favorites";
import {
  NOW_PLAYING_READY_EVENT,
  openNowPlayingWindow,
  publishNowPlayingSnapshot,
} from "./services/nowPlayingWindow";
import { plexCollectionCacheKey } from "./services/plexArtworkCache";
import { listen } from "@tauri-apps/api/event";
import {
  PlexLibrary,
  PlexAlbum,
  PlexCollection,
  SelectedArtist,
  PlexSearchResults,
  PlexImageRef,
} from "./types/plex";
import { PlaybackStatus, AudioDevice, MpdHealth } from "./types/audio";
import { AppConfig } from "./types/config";
import { FavoriteAlbum } from "./types/favorite";
import type { NewPlaylistItem } from "./types/playlist";
import {
  isTextPlexAlbumSort,
  plexAlbumBucketFirstIndices,
  plexAlbumNavigationResetKey,
  sortPlexAlbums,
  type PlexAlbumSortMode,
} from "./utils/plexAlbumNavigation";
import { LOCAL_ALBUM_INDEX_BUCKETS } from "./utils/localAlbumNavigation";
import { currentMediaKey, PlayerNavigationRequests, visitPlayerTarget, type PlayerNavigationTarget, type PlayerNavigationSession } from "./utils/playerNavigation";
import {
  albumsForPlexArtist,
  derivePlexArtists,
  plexArtistBucketFirstIndices,
  sortPlexArtists,
  type PlexArtistSortMode,
  type PlexArtistSummary,
} from "./utils/plexArtistNavigation";
import {
  derivePlexYearNavigation,
  plexAlbumsForYear,
  plexAlbumsWithoutYear,
} from "./utils/plexYearNavigation";

interface CollectionAlbumsCacheEntry {
  promise: Promise<PlexAlbum[]>;
  state: "pending" | "fulfilled" | "rejected";
}

const collectionAlbumsCache = new Map<string, CollectionAlbumsCacheEntry>();

const getCachedCollectionAlbums = (
  serverId: string,
  ratingKey: string,
  retryRejected = false,
): Promise<PlexAlbum[]> => {
  const cacheKey = plexCollectionCacheKey(serverId, ratingKey);
  const cached = collectionAlbumsCache.get(cacheKey);
  if (cached && (!retryRejected || cached.state !== "rejected")) {
    return cached.promise;
  }

  const request = plexService.getCollectionAlbums(ratingKey);
  const entry: CollectionAlbumsCacheEntry = { promise: request, state: "pending" };
  collectionAlbumsCache.set(cacheKey, entry);
  void request.then(
    () => {
      entry.state = "fulfilled";
    },
    () => {
      entry.state = "rejected";
    },
  );
  return request;
};

const PlexCollectionArtwork = memo<{ collection: PlexCollection }>(({ collection }) => {
  const artworkRef = useRef<HTMLDivElement>(null);
  const [primaryFailed, setPrimaryFailed] = useState(false);
  const [mosaicThumbs, setMosaicThumbs] = useState<PlexImageRef[]>([]);
  const [failedMosaicSlots, setFailedMosaicSlots] = useState<Set<number>>(new Set());
  const showPrimary = Boolean(collection.thumb) && !primaryFailed;

  useEffect(() => {
    if (showPrimary) return;

    let disposed = false;
    let requested = false;
    let observer: IntersectionObserver | undefined;

    const loadMosaic = () => {
      if (requested) return;
      requested = true;
      getCachedCollectionAlbums(collection.server_id, collection.rating_key)
        .then((albums) => {
          if (disposed) return;
          const thumbs = albums
            .filter((album): album is PlexAlbum & { thumb: PlexImageRef } => Boolean(album.thumb))
            .sort((a, b) => a.rating_key.localeCompare(b.rating_key))
            .map((album) => album.thumb)
            .slice(0, 4);
          setMosaicThumbs(thumbs);
          setFailedMosaicSlots(new Set());
        })
        .catch((err) => {
          if (!disposed) logError("plex.collection-artwork", err);
        });
    };

    const node = artworkRef.current;
    if (node && "IntersectionObserver" in window) {
      observer = new IntersectionObserver((entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer?.disconnect();
          loadMosaic();
        }
      });
      observer.observe(node);
    } else {
      loadMosaic();
    }

    return () => {
      disposed = true;
      observer?.disconnect();
    };
  }, [collection.rating_key, collection.server_id, showPrimary]);

  if (showPrimary) {
    return (
      <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md">
        <PlexImage
          image={collection.thumb!}
          alt={collection.title}
          className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
          loading="lazy"
          onLoadError={() => setPrimaryFailed(true)}
        />
      </div>
    );
  }

  return (
    <div
      ref={artworkRef}
      className="relative aspect-square w-full rounded-lg bg-[#202020] border border-[#2B2B2B] overflow-hidden mb-2.5 shadow-md group-hover:border-[#E5A00D] transition-colors"
    >
      <div className="grid grid-cols-2 grid-rows-2 w-full h-full gap-px bg-[#2B2B2B]">
        {[0, 1, 2, 3].map((slot) => {
          const thumb = mosaicThumbs[slot];
          return thumb && !failedMosaicSlots.has(slot) ? (
            <PlexImage
              key={`${slot}-${thumb.server_id}-${thumb.path}`}
              image={thumb}
              alt=""
              className="w-full h-full object-cover"
              loading="lazy"
              onLoadError={() => {
                setFailedMosaicSlots((current) => new Set(current).add(slot));
              }}
            />
          ) : (
            <div key={slot} className="w-full h-full bg-[#1B1B1B] flex items-center justify-center">
              <Disc3 size={18} className="text-[#444444]" />
            </div>
          );
        })}
      </div>
    </div>
  );
});

PlexCollectionArtwork.displayName = "PlexCollectionArtwork";

// Card isolado e memorizado para evitar re-render a cada 1s da telemetria do player
const PlexAlbumCard = memo<{
  album: PlexAlbum;
  isFav: boolean;
  onSelect: (album: PlexAlbum) => void;
  onPlayQuick: (e: React.MouseEvent, album: PlexAlbum) => void;
  onToggleFav: (e: React.MouseEvent, album: PlexAlbum) => void;
  onSelectArtist: (artistKey: string, artistName: string) => void;
  removeFavText: string;
  addFavText: string;
  playAlbumText: string;
  isPlaybackAvailable: boolean;
}>(({
  album,
  isFav,
  onSelect,
  onPlayQuick,
  onToggleFav,
  onSelectArtist,
  removeFavText,
  addFavText,
  playAlbumText,
  isPlaybackAvailable,
}) => {
  return (
    <div
      onClick={() => onSelect(album)}
      className="group flex flex-col cursor-pointer relative"
    >
      <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md">
        {album.thumb ? (
          <PlexImage
            image={album.thumb}
            alt={album.title}
            className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
            loading="lazy"
          />
        ) : (
          <div className="w-full h-full flex items-center justify-center text-[#444444]">
            <Disc3 size={40} />
          </div>
        )}

        <button
          onClick={(e) => onToggleFav(e, album)}
          className={`absolute top-2 right-2 p-1.5 rounded-full backdrop-blur-xs transition-transform active:scale-90 cursor-pointer shadow z-20 ${
            isFav
              ? "bg-black/60 text-[#E5A00D]"
              : "bg-black/40 text-white/70 hover:text-white opacity-0 group-hover:opacity-100"
          }`}
          title={isFav ? removeFavText : addFavText}
        >
          <Heart size={14} fill={isFav ? "#E5A00D" : "none"} />
        </button>

        <div className="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center pointer-events-none">
          <button
            onClick={(e) => onPlayQuick(e, album)}
            disabled={!isPlaybackAvailable}
            className="w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer pointer-events-auto disabled:opacity-50 disabled:cursor-not-allowed"
            title={isPlaybackAvailable ? playAlbumText : undefined}
          >
            <Play size={20} className="ml-1" fill="black" />
          </button>
        </div>
      </div>

      <span className="text-sm font-semibold text-white truncate" title={album.title}>
        {album.title}
      </span>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          const artistKey = album.artist_rating_key?.trim();
          if (artistKey) {
            onSelectArtist(artistKey, album.artist);
          }
        }}
        className="text-xs text-[#999999] hover:text-[#E5A00D] transition-colors truncate mt-0.5 text-left cursor-pointer"
      >
        {album.artist}
      </button>
      {album.year && (
        <span className="text-[11px] text-[#666666] mt-0.5">
          {album.year}
        </span>
      )}
    </div>
  );
});

PlexAlbumCard.displayName = "PlexAlbumCard";

export function App() {
  const { t } = useTranslation();
  const localCatalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const [showCollectionWizard, setShowCollectionWizard] = useState(false);
  const collectionWizardWasOpen = useRef(false);
  useEffect(() => {
    if (!showCollectionWizard && collectionWizardWasOpen.current) document.getElementById("explore-configure-collection")?.focus();
    collectionWizardWasOpen.current = showCollectionWizard;
  }, [showCollectionWizard]);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [mediaSource, setMediaSource] = useState<MainSection | null>(null);
  const [playerNavigation, setPlayerNavigation] = useState<PlayerNavigationSession | null>(null);
  const playerTarget = playerNavigation?.target ?? null;
  const [exploreDetail, setExploreDetail] = useState<ExploreEntry | null>(null);
  const [exploreLocalSubview, setExploreLocalSubview] = useState<LocalSubview>("browser");
  const [localSubview, setLocalSubview] = useState<LocalSubview>("browser");
  const [playerLocalSubview, setPlayerLocalSubview] = useState<{ revision: number; view: LocalSubview } | null>(null);
  const playerSubview = playerLocalSubview?.revision === playerNavigation?.revision ? playerLocalSubview?.view ?? "browser" : "browser";
  const changePlayerLocalSubview = (view: LocalSubview) => setPlayerLocalSubview({ revision: playerNavigation?.revision ?? 0, view });
  const playerReturnSection = playerNavigation?.returnSection ?? null;
  const setPlayerTarget = (target: PlayerNavigationTarget | null) => {
    setPlayerNavigation(previous => previous && target ? { ...previous, target, revision: previous.revision + 1 } : null);
  };
  const [playerNavigationError, setPlayerNavigationError] = useState(false);
  const playerNavigationRequests = useMemo(() => new PlayerNavigationRequests(), []);
  // Keep the previous view (including its context and scroll) mounted while visiting the player target.
  const centralSection = playerReturnSection ?? mediaSource;
  const mainSectionPreferenceRef = useRef<ReturnType<typeof createMainSectionPreference> | null>(null);
  if (!mainSectionPreferenceRef.current) {
    mainSectionPreferenceRef.current = createMainSectionPreference(setMediaSource);
  }
  const mainSectionPreference = mainSectionPreferenceRef.current;
  const [bootstrapError, setBootstrapError] = useState(false);
  const [managedSavePending, setManagedSavePending] = useState(false);
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);
  const [localSelectedArtist, setLocalSelectedArtist] = useState<string | null>(null);
  const [localCatalogRevision, setLocalCatalogRevision] = useState(0);
  const [libraries, setLibraries] = useState<PlexLibrary[]>([]);
  const [loadingLibraries, setLoadingLibraries] = useState(true);
  const [selectedLibrary, setSelectedLibrary] = useState<PlexLibrary | null>(null);
  // Opaque identities prevent credential values from reaching cards, errors or persisted navigation.
  const plexConnectionIdentity = useMemo(() => ({}), [config?.plex_server_id, config?.plex_url, config?.plex_token]);
  const explorePlexIdentity = useMemo(() => ({}), [plexConnectionIdentity, selectedLibrary?.key]);
  const [plexLibrariesIdentity, setPlexLibrariesIdentity] = useState<unknown>(null);
  const [plexLibrariesError, setPlexLibrariesError] = useState(false);
  const [plexDiscoveryAttempt, setPlexDiscoveryAttempt] = useState(0);
  const plexCatalog = useMemo(() => new PlexCatalogCoordinator(
    libraryId => plexService.getAlbums(libraryId, "added"), error => logError("plex.albums-load", error)), []);
  const artistPortraits = useMemo(() => new PlexArtistPortraitCache(plexService.getArtistPortrait), []);
  const plexAggregate = useSyncExternalStore(plexCatalog.subscribe, plexCatalog.getSnapshot);
  useEffect(() => {
    plexCatalog.setContext(plexConnectionIdentity, config?.plex_server_id ?? "");
    artistPortraits.setContext(plexConnectionIdentity, config?.plex_server_id ?? "");
    return () => { plexCatalog.invalidate(plexConnectionIdentity); artistPortraits.invalidate(plexConnectionIdentity); };
  }, [plexCatalog, plexConnectionIdentity, artistPortraits]);
  useEffect(() => {
    if (centralSection === "explore" && plexLibrariesIdentity === plexConnectionIdentity && !loadingLibraries && !plexLibrariesError) plexCatalog.aggregate(libraries);
  }, [centralSection, plexCatalog, libraries, plexLibrariesIdentity, plexConnectionIdentity, loadingLibraries, plexLibrariesError]);

  const [plexAlbumLoad, setPlexAlbumLoad] = useState<{ identity: unknown; phase: "loading" | "ready" | "error" } | null>(null);
  const [plexViewMode, setPlexViewMode] = useState<"albums" | "artists" | "years" | "genres" | "collections">("albums");
  const [hasCollections, setHasCollections] = useState(false);
  const [plexSortMode, setPlexSortMode] = useState<PlexAlbumSortMode>("added_recent");
  const [selectedPlexGenre, setSelectedPlexGenre] = useState<string | null>(null);
  const [plexArtistSortMode, setPlexArtistSortMode] = useState<PlexArtistSortMode>("artist_asc");
  const [showSettings, setShowSettings] = useState(false);
  const queueClearFlowRef = useRef(createQueueClearConfirmationFlow());
  const [queueClearConfirmation, setQueueClearConfirmation] = useState<QueueClearConfirmation | null>(null);
  const configSaveInProgressRef = useRef(false);
  const configSaveGenerationRef = useRef(0);
  const [configSavePhase, setConfigSavePhase] = useState<"local" | "other" | null>(null);
  const [localSavePhase, setLocalSavePhase] = useState<ConfigSavePhase | null>(null);
  const [importPreview, setImportPreview] = useState<PreviewAlbum[]>([]);
  const importPreviewRef = useRef<ReturnType<typeof createLocalImportPreview> | null>(null);
  const configSaveMonitorRef = useRef<ReturnType<typeof monitorConfigSave> | null>(null);
  const [configSaveNotice, setConfigSaveNotice] = useState<{
    kind: "success" | "error";
    detail?: string;
    reloadFailed?: boolean;
    refusal?: "fullRescan" | "planChanged" | "queueChanged" | "queueCheckRequired";
    stagingCleanupFailed?: boolean;
    uncertain?: boolean;
    statusUnavailable?: boolean;
  } | null>(null);
  // Expire only visible success notices; progress and failures stay explicit.
  useEffect(() => {
    if (configSavePhase || configSaveNotice?.kind !== "success" || configSaveNotice.uncertain || configSaveNotice.statusUnavailable) return;
    const notice = configSaveNotice;
    const generation = configSaveGenerationRef.current;
    let active = true;
    const timer = window.setTimeout(() => {
      if (!active || configSaveInProgressRef.current || generation !== configSaveGenerationRef.current) return;
      setConfigSaveNotice(current => current === notice ? null : current);
    }, 30_000);
    return () => { active = false; window.clearTimeout(timer); };
  }, [configSaveNotice, configSavePhase]);
  const [showAbout, setShowAbout] = useState(false);

  // Favoritos Plex (IDs em cache)
  const [plexFavIds, setPlexFavIds] = useState<Set<string>>(new Set());

  // Fila e status global
  const [showQueue, setShowQueue] = useState(false);
  const [playbackStatus, setPlaybackStatus] = useState<PlaybackStatus>({
    state: "stop",
    elapsed: 0.0,
    duration: 0.0,
    audio_format: "",
    current_media: null,
    title: "",
    artist: "",
    album: "",
    thumb: null,
    plex_image: null,
    volume: {
      value: 100,
      muted: false,
      writable: true,
      available: true,
      backend: "unavailable",
    },
    is_updating: false,
  });
  const [mpdHealth, setMpdHealth] = useState<MpdHealth>({ state: "starting" });
  const isPlaybackAvailable = mpdHealth.state === "available";
  const statusRequestGenerationRef = useRef(0);
  const plexLibraryRequestGenerationRef = useRef(0);
  const plexSearchRequestGenerationRef = useRef(0);
  const plexGridRef = useRef<VirtualAlbumGridHandle>(null);
  const plexArtistListRef = useRef<VirtualListHandle>(null);
  const [plexScrollContainer, setPlexScrollContainer] = useState<HTMLDivElement | null>(null);

  const statusRef = useRef(playbackStatus);
  useLayoutEffect(() => {
    // Publish the artwork gate before child passive effects start cover queries.
    const blocked = artworkReadBlocked(playbackStatus.is_updating, localCatalog);
    localArtworkResolver.setContext(blocked, localCatalog.albums);
    localArtworkEnrichment.setUpdating(blocked);
  }, [playbackStatus.is_updating, localCatalog]);
  useEffect(() => {
    localArtworkEnrichment.setOnlineEnabled(config?.online_artwork_enabled ?? false);
  }, [config?.online_artwork_enabled]);
  useEffect(() => {
    statusRef.current = playbackStatus;
  }, [playbackStatus]);
  const playbackAvailableRef = useRef(isPlaybackAvailable);
  useEffect(() => {
    playbackAvailableRef.current = isPlaybackAvailable;
  }, [isPlaybackAvailable]);
  const nowPlayingReadyRef = useRef(false);
  const nowPlayingSnapshotRef = useRef({
    route: configuredSignalRoute(config),
    health: mpdHealth,
    playback: playbackStatus,
  });

  const publishCurrentNowPlayingSnapshot = useCallback(() => {
    if (!nowPlayingReadyRef.current) return;
    void publishNowPlayingSnapshot(nowPlayingSnapshotRef.current).catch(() => {
      nowPlayingReadyRef.current = false;
    });
  }, []);

  const invalidateStatusRequests = useCallback(() => {
    statusRequestGenerationRef.current += 1;
    setMpdHealth({ state: "starting" });
  }, []);

  // Listagens Plex
  const [loadedBrowserAlbums, setAlbums] = useState<PlexAlbum[]>([]);
  // A retry completed in Explore is immediately visible in the browser too.
  const albums = plexAggregate.identity === plexConnectionIdentity
    ? plexAggregate.libraries.find(item => item.library.key === selectedLibrary?.key && item.phase === "complete")?.albums ?? loadedBrowserAlbums
    : [];
  const [collections, setCollections] = useState<PlexCollection[]>([]);

  // Navegação interna Plex
  const [activeCollection, setActiveCollection] = useState<PlexCollection | null>(null);
  const [collectionAlbums, setCollectionAlbums] = useState<PlexAlbum[]>([]);
  const [activeAlbum, setActiveAlbum] = useState<PlexAlbum | null>(null);
  const [activeArtist, setActiveArtist] = useState<SelectedArtist | null>(null);
  const [fallbackPlexArtist, setFallbackPlexArtist] = useState<PlexArtistSummary | null>(null);
  const [selectedPlexDecade, setSelectedPlexDecade] = useState<number | null>(null);
  const [selectedPlexYear, setSelectedPlexYear] = useState<number | null>(null);
  const [selectedPlexUnknownYear, setSelectedPlexUnknownYear] = useState(false);

  // Busca Global
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<PlexSearchResults | null>(null);
  const [isSearching, setIsSearching] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const [loading, setLoading] = useState(false);

  useLayoutEffect(() => {
    playerNavigationRequests.setContext(JSON.stringify([
      currentMediaKey(playbackStatus.current_media), mediaSource, selectedLibrary?.key,
      activeAlbum?.rating_key, activeArtist?.rating_key, activeCollection?.rating_key,
      plexViewMode, searchQuery, config?.plex_server_id,
    ]));
  }, [playbackStatus.current_media, mediaSource, selectedLibrary, activeAlbum, activeArtist,
    activeCollection, plexViewMode, searchQuery, config?.plex_server_id, playerNavigationRequests]);
  useLayoutEffect(() => { playerNavigationRequests.invalidate(); }, [localCatalog.albums,
    localCatalog.saveBlocked, localCatalog.loading, localCatalog.mpdBusy, localCatalog.error,
    playbackStatus.is_updating, mpdHealth.state, config?.plex_token, playerNavigationRequests]);
  useEffect(() => () => playerNavigationRequests.invalidate(), [playerNavigationRequests]);

  const closePlayerNavigation = () => {
    playerNavigationRequests.invalidate();
    setPlayerNavigation(null);
    if (playerReturnSection) mainSectionPreference.navigate(playerReturnSection);
  };

  useEffect(() => {
    if (playerTarget?.kind === "plex") closePlayerNavigation();
  }, [config?.plex_server_id, config?.plex_token]);

  const navigateFromPlayer = (view: "album" | "artist") => {
    setPlayerNavigationError(false);
    void playerNavigationRequests.open(
      playbackStatus.current_media, view,
      localCatalog.loaded && !localCatalog.loading && !localCatalog.saveBlocked && !localCatalog.mpdBusy
        && !localCatalog.error && !playbackStatus.is_updating ? localCatalog.albums : [],
      plexService.resolvePlayerNavigation,
      target => {
        setPlayerNavigation(previous => visitPlayerTarget(previous, target, centralSection ?? target.kind));
        mainSectionPreference.navigate(target.kind);
      },
      () => setPlayerNavigationError(true),
      playbackStatus.artist,
      audioService.getLocalAlbumTracks,
    );
  };

  const refreshPlexFavorites = useCallback(() => {
    favoritesService
      .getFavorites()
      .then((favs) => {
        const ids = favs
          .filter((f) => f.source === "plex" && f.id && f.id.trim().length > 0)
          .map((f) => String(f.id));
        setPlexFavIds(new Set(ids));
      })
      .catch(error => logError("favorites.load", error));
  }, []);

  const beginConfigSave = (phase: "local" | "other"): boolean => {
    if (configSaveInProgressRef.current) return false;
    if (queueClearFlowRef.current.isPending()) return false;
    configSaveGenerationRef.current += 1;
    configSaveInProgressRef.current = true;
    if (phase === "local") localAlbumCatalog.setSaveBlocked(true);
    setConfigSavePhase(phase);
    setConfigSaveNotice(null);
    return true;
  };

  const finishConfigSave = () => {
    configSaveInProgressRef.current = false;
    setConfigSavePhase(null);
    setLocalSavePhase(null);
  };

  const reportLocalFoldersSaveFailure = async (error: unknown, uncertain = false, isCurrent = () => true) => {
    const detail = formatConfigError(error);
    const [reason, cleanup] = detail.split(";");
    const refusal = reason === "LOCAL_SOURCE_FULL_RESCAN_REQUIRED" ? "fullRescan"
      : reason === "LOCAL_SOURCE_TARGETED_PLAN_CHANGED" ? "planChanged"
      : !uncertain && reason === "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE" ? "queueChanged"
      : !uncertain && queueClearBinding(detail) ? "queueCheckRequired" : undefined;
    logError("settings.local-folders-save", error);
    let reloadFailed = false;
    try {
      const confirmed = await configService.getConfig();
      if (!isCurrent()) return;
      setConfig(confirmed);
      if (!uncertain) localAlbumCatalog.setSaveBlocked(false);
    } catch (reloadError) {
      reloadFailed = true;
      logError("settings.reload-confirmed", reloadError);
    }
    if (!isCurrent()) return;
    setConfigSaveNotice({
      kind: "error",
      detail: refusal ? undefined : detail,
      reloadFailed,
      refusal,
      stagingCleanupFailed: Boolean(refusal) && cleanup === "LOCAL_SOURCE_STAGING_CLEANUP_FAILED",
      uncertain,
    });
  };

  useEffect(() => {
    let disposed = false;
    const preview = createLocalImportPreview(configService.getLocalImportPreview, setImportPreview);
    importPreviewRef.current = preview;
    const monitor = monitorConfigSave(configService.getSaveOperation, (operation) => {
      preview.observe(operation);
      const generation = ++configSaveGenerationRef.current;
      const isCurrent = () => !disposed && generation === configSaveGenerationRef.current;
      const confirmation = queueClearFlowRef.current.observe(operation);
      if (operation.phase === "uncertain") localAlbumCatalog.setSaveBlocked(true);
      setQueueClearConfirmation(null);
      if (configSavePending(operation)) {
        localAlbumCatalog.setSaveBlocked(true);
        configSaveInProgressRef.current = true;
        setConfigSavePhase("local");
        setLocalSavePhase(operation.phase);
        setConfigSaveNotice(null);
        return;
      }
      void (async () => {
        try {
          if (confirmation) {
            localAlbumCatalog.setSaveBlocked(false);
            setQueueClearConfirmation(confirmation);
            setConfigSaveNotice(null);
          } else if (operation.phase === "completed") {
            const confirmed = await configService.getConfig();
            if (!isCurrent()) return;
            setConfig(confirmed);
            localAlbumCatalog.setSaveBlocked(false);
            setLocalCatalogRevision((revision) => revision + 1);
            setConfigSaveNotice({ kind: "success" });
          } else {
            await reportLocalFoldersSaveFailure(operation.error ?? operation.phase, operation.phase === "uncertain", isCurrent);
          }
        } catch (error) {
          if (isCurrent()) await reportLocalFoldersSaveFailure(error, true, isCurrent);
        } finally {
          if (isCurrent()) finishConfigSave();
        }
      })();
    }, (error) => {
      logError("settings.save-status", error);
      if (configSaveMonitorRef.current?.isPending()) {
        // Losing the observer is not evidence of failure/completion. Keep the
        // pending operation and continue observing without enabling another Save.
        setConfigSaveNotice({ kind: "error", statusUnavailable: true });
      }
    });
    configSaveMonitorRef.current = monitor;
    return () => {
      disposed = true;
      monitor.dispose();
      preview.dispose();
      importPreviewRef.current = null;
      configSaveMonitorRef.current = null;
      queueClearFlowRef.current.discard();
    };
  }, []);

  const startLocalFoldersSave = (nextConfig: AppConfig, authorization?: QueueClearAuthorization): boolean => {
    const monitor = configSaveMonitorRef.current;
    if (!monitor || !beginConfigSave("local")) return false;
    const generation = configSaveGenerationRef.current;
    const isCurrent = () => generation === configSaveGenerationRef.current && configSaveMonitorRef.current === monitor;
    importPreviewRef.current?.clear();
    const proposal = queueClearFlowRef.current.begin(nextConfig);
    setQueueClearConfirmation(null);
    setLocalSavePhase("preparing");
    monitor.beginAcceptance();
    void (async () => {
      try {
        const response = await configService.saveConfig(proposal, authorization);
        if (!isCurrent()) return;
        if (response.status === "accepted") {
          queueClearFlowRef.current.accepted(response.operation.id);
          monitor.accepted(response.operation);
        } else {
          queueClearFlowRef.current.discard();
          monitor.acceptanceFailed();
          const confirmed = await configService.getConfig();
          if (!isCurrent()) return;
          setConfig(confirmed);
          localAlbumCatalog.setSaveBlocked(false);
          setLocalCatalogRevision((revision) => revision + 1);
          setConfigSaveNotice({ kind: "success" });
          finishConfigSave();
        }
      } catch (error) {
        if (!isCurrent()) return;
        queueClearFlowRef.current.discard();
        monitor.acceptanceFailed();
        await reportLocalFoldersSaveFailure(error, false, isCurrent);
        if (isCurrent()) finishConfigSave();
      }
    })();
    return true;
  };

  const cancelQueueClear = (operationId: string) => {
    if (configSaveInProgressRef.current) return;
    if (!queueClearFlowRef.current.cancel(operationId)) return;
    setQueueClearConfirmation(null);
    setShowSettings(true);
  };

  const confirmQueueClear = (operationId: string) => {
    if (configSaveInProgressRef.current) return;
    const request = queueClearFlowRef.current.confirm(operationId);
    if (!request) return;
    setQueueClearConfirmation(null);
    startLocalFoldersSave(request.config, request.authorization);
  };

  // Carga inicial de configurações e dispositivos
  useEffect(() => {
    let disposed = false;
    Promise.all([configService.getConfig(), configService.getAudioDevices()])
      .then(([cfg, devs]) => {
        if (disposed) return;
        setConfig(cfg);
        setDevices(devs);
        mainSectionPreference.restore(cfg);
      })
      .catch((error) => {
        if (!disposed) {
          logError("app.bootstrap", error);
          setManagedSavePending(formatConfigError(error).includes("MANAGED_FOLDERS_SAVE_PENDING"));
          setBootstrapError(true);
        }
      });
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen(NOW_PLAYING_READY_EVENT, () => {
      if (disposed) return;
      nowPlayingReadyRef.current = true;
      publishCurrentNowPlayingSnapshot();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch((err) => {
      if (!disposed) logError("app.now-playing-listener", err);
    });

    return () => {
      disposed = true;
      nowPlayingReadyRef.current = false;
      unlisten?.();
    };
  }, [publishCurrentNowPlayingSnapshot]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen(LOCAL_LIBRARY_RECONCILED_EVENT, () => {
      if (!disposed) localAlbumCatalog.requestRefresh();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch((error) => {
      if (!disposed) logError("local.reconciliation-listener", error);
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    nowPlayingSnapshotRef.current = {
      route: configuredSignalRoute(config),
      health: mpdHealth,
      playback: playbackStatus,
    };
    publishCurrentNowPlayingSnapshot();
  }, [config, mpdHealth, playbackStatus, publishCurrentNowPlayingSnapshot]);

  // Atualiza favoritos Plex ao alternar a fonte de mídia
  useEffect(() => {
    refreshPlexFavorites();
  }, [mediaSource, refreshPlexFavorites]);

  // Fonte única de saúde e telemetria do MPD (polling encadeado, sem sobreposição).
  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    const update = async () => {
      const generation = statusRequestGenerationRef.current + 1;
      statusRequestGenerationRef.current = generation;
      try {
        const snapshot = await audioService.getMpdStatusSnapshot();
        if (disposed || generation !== statusRequestGenerationRef.current) return;

        if (snapshot.health.state === "available" && snapshot.playback) {
          localAlbumCatalog.onMpdStatus(snapshot.playback.is_updating);
          setMpdHealth(snapshot.health);
          setPlaybackStatus(snapshot.playback);
        } else if (snapshot.health.state === "available") {
          setMpdHealth({ state: "unavailable", reason: "protocol_unavailable" });
        } else {
          setMpdHealth(snapshot.health);
        }
      } catch (err) {
        if (!disposed && generation === statusRequestGenerationRef.current) {
          setMpdHealth({ state: "unavailable", reason: "protocol_unavailable" });
          logError("audio.health", err);
        }
      } finally {
        if (!disposed) {
          timer = window.setTimeout(update, 1000);
        }
      }
    };

    void update();
    return () => {
      disposed = true;
      statusRequestGenerationRef.current += 1;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, []);

  // Título Dinâmico da Janela
  useEffect(() => {
    if (isPlaybackAvailable && playbackStatus.state === "play" && playbackStatus.title) {
      const trackLabel = playbackStatus.artist
        ? `${playbackStatus.artist} — ${playbackStatus.title}`
        : playbackStatus.title;
      audioService.setWindowTitle(`Sonante • ${trackLabel}`).catch(error => logError("app.window-title", error));
    } else {
      audioService.setWindowTitle("Sonante").catch(error => logError("app.window-title", error));
    }
  }, [isPlaybackAvailable, playbackStatus.state, playbackStatus.title, playbackStatus.artist]);

  // Atalhos de Teclado Globais
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const activeEl = document.activeElement;
      const isInput =
        activeEl?.tagName === "INPUT" ||
        activeEl?.tagName === "SELECT" ||
        activeEl?.tagName === "TEXTAREA";

      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        if (playerTarget) return; // The previous search field is hidden while visiting the player target.
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
        return;
      }

      if (e.key === "Escape") {
        // The Local dialog owns Escape, including draft/result confirmations and Apply guards.
        if (document.querySelector("[data-local-tag-dialog-open]")) return;
        playerNavigationRequests.invalidate();
        if (showAbout) {
          setShowAbout(false);
        } else if (showSettings) {
          setShowSettings(false);
        } else if (showQueue) {
          setShowQueue(false);
        } else if (playerTarget) {
          closePlayerNavigation();
        } else if (exploreDetail) {
          closeExploreAlbum();
        } else if (searchQuery) {
          setSearchQuery("");
        } else if (activeEl instanceof HTMLElement) {
          activeEl.blur();
        }
        return;
      }

      if (isInput) return;

      if (!playbackAvailableRef.current) return;

      const current = statusRef.current;

      if (e.code === "Space") {
        e.preventDefault();
        audioService.togglePlay().catch(error => logError("player.toggle", error));
      } else if (e.code === "ArrowRight") {
        e.preventDefault();
        playerNavigationRequests.invalidate();
        audioService.next().catch(error => logError("player.next", error));
      } else if (e.code === "ArrowLeft") {
        e.preventDefault();
        playerNavigationRequests.invalidate();
        audioService.previous().catch(error => logError("player.previous", error));
      } else if (e.code === "ArrowUp") {
        e.preventDefault();
        if (!current.volume.available || !current.volume.writable) return;
        const nextVol = Math.min(current.volume.value + 5, 100);
        audioService.setVolume(nextVol).catch(error => logError("player.volume", error));
      } else if (e.code === "ArrowDown") {
        e.preventDefault();
        if (!current.volume.available || !current.volume.writable) return;
        const prevVol = Math.max(current.volume.value - 5, 0);
        audioService.setVolume(prevVol).catch(error => logError("player.volume", error));
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        if (!current.volume.available || !current.volume.writable) return;
        const currentVol = current.volume.value;
        audioService
          .setVolume(!current.volume.muted && currentVol > 0 ? 0 : currentVol || 100)
          .catch(error => logError("player.volume", error));
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [showSettings, showAbout, showQueue, searchQuery, playerTarget, exploreDetail]);

  // Carregar ou Limpar Bibliotecas do Plex
  useEffect(() => {
    if (!config?.plex_token || config.plex_token.trim().length === 0) {
      mainSectionPreference.plexDiscoveryFinished(false);
      setLoadingLibraries(false);
      setLibraries([]);
      setSelectedLibrary(null);
      setAlbums([]);
      setCollections([]);
      setActiveAlbum(null);
      setActiveCollection(null);
      setActiveArtist(null);
      setFallbackPlexArtist(null);
      setSelectedPlexDecade(null);
      setSelectedPlexYear(null);
      setSelectedPlexUnknownYear(false);
      setSelectedPlexGenre(null);
      return;
    }

    let disposed = false;
    setPlexLibrariesError(false);
    setLibraries([]);
    setSelectedLibrary(null);
    setLoadingLibraries(true);
    plexService
      .getLibraries()
      .then((libs) => {
        if (disposed) return;
        mainSectionPreference.plexDiscoveryFinished(libs.length > 0);
        setPlexLibrariesIdentity(plexConnectionIdentity);
        setLibraries(libs);
        if (libs.length > 0) {
          setSelectedLibrary(libs[0]);
        } else {
          setSelectedLibrary(null);
        }
      })
      .catch((err) => {
        if (!disposed) {
          mainSectionPreference.plexDiscoveryFinished(false);
          setPlexLibrariesIdentity(plexConnectionIdentity);
          setPlexLibrariesError(true);
          logError("plex.libraries-load", err);
        }
      })
      .finally(() => {
        if (!disposed) setLoadingLibraries(false);
      });

    return () => {
      disposed = true;
    };
  }, [plexConnectionIdentity, plexDiscoveryAttempt]);

  // Carregar o catálogo base e as coleções da biblioteca ativa uma única vez.
  useEffect(() => {
    if (!selectedLibrary || plexLibrariesIdentity !== plexConnectionIdentity) {
      setAlbums([]);
      setCollections([]);
      setHasCollections(false);
      setLoading(false);
      return;
    }
    const generation = plexLibraryRequestGenerationRef.current + 1;
    plexLibraryRequestGenerationRef.current = generation;
    let disposed = false;

    setActiveAlbum(null);
    setActiveCollection(null);
    setActiveArtist(null);
    setFallbackPlexArtist(null);
    setSelectedPlexDecade(null);
    setSelectedPlexYear(null);
    setSelectedPlexUnknownYear(false);
    setSelectedPlexGenre(null);
    setSearchQuery("");
    setSearchResults(null);
    // Keep valid same-context inventory on failure; never display another scope.
    if (plexAlbumLoad && plexAlbumLoad.identity !== explorePlexIdentity) setAlbums([]);
    setCollections([]);
    setHasCollections(false);
    setLoading(true);
    setPlexAlbumLoad({ identity: explorePlexIdentity, phase: "loading" });
    void Promise.allSettled([
      plexCatalog.loadLibrary(selectedLibrary),
      plexService.getCollections(selectedLibrary.key),
    ]).then(([albumResult, collectionResult]) => {
      if (disposed || generation !== plexLibraryRequestGenerationRef.current) return;
      if (albumResult.status === "fulfilled") {
        setAlbums(albumResult.value);
        setPlexAlbumLoad({ identity: explorePlexIdentity, phase: "ready" });
      } else {
        setPlexAlbumLoad({ identity: explorePlexIdentity, phase: "error" });
        logError("plex.albums-load", albumResult.reason);
      }
      if (collectionResult.status === "fulfilled") {
        const loadedCollections = collectionResult.value;
        setCollections(loadedCollections);
        setHasCollections(loadedCollections.length > 0);
        if (loadedCollections.length === 0) {
          setPlexViewMode((current) => current === "collections" ? "albums" : current);
        }
      } else {
        setHasCollections(false);
        setPlexViewMode((current) => current === "collections" ? "albums" : current);
        logError("plex.collections-load", collectionResult.reason);
      }
      setLoading(false);
    });

    return () => {
      disposed = true;
    };
  }, [selectedLibrary, plexLibrariesIdentity, plexConnectionIdentity, explorePlexIdentity, plexCatalog]);

  // Busca Plex
  useEffect(() => {
    const q = searchQuery.trim();
    const generation = plexSearchRequestGenerationRef.current + 1;
    plexSearchRequestGenerationRef.current = generation;
    if (q.length === 0 || centralSection !== "plex") {
      setSearchResults(null);
      setIsSearching(false);
      return;
    }

    setIsSearching(true);
    const timer = setTimeout(() => {
      plexService
        .search(q, selectedLibrary?.key)
        .then((res) => {
          if (generation === plexSearchRequestGenerationRef.current) setSearchResults(res);
        })
        .catch((error) => {
          if (generation === plexSearchRequestGenerationRef.current) logError("plex.search", error);
        })
        .finally(() => {
          if (generation === plexSearchRequestGenerationRef.current) setIsSearching(false);
        });
    }, 300);

    return () => {
      clearTimeout(timer);
      if (generation === plexSearchRequestGenerationRef.current) {
        plexSearchRequestGenerationRef.current += 1;
      }
    };
  }, [searchQuery, selectedLibrary, centralSection]);

  const handleSelectCollection = async (col: PlexCollection) => {
    setActiveCollection(col);
    setLoading(true);
    try {
      const items = await getCachedCollectionAlbums(col.server_id, col.rating_key, true);
      setCollectionAlbums(items);
    } catch (err) {
      logError("plex.collection-albums", err);
    } finally {
      setLoading(false);
    }
  };

  const handlePlayQuick = useCallback(async (e: React.MouseEvent, album: PlexAlbum) => {
    e.stopPropagation();
    if (!playbackAvailableRef.current) return;
    try {
      const tracks = await plexService.getAlbumTracks(album.rating_key);
      const metaTracks = tracks.map((t) => ({
        title: t.title,
        artist: album.artist,
        album: album.title,
        plex_image: t.thumb || album.thumb || null,
        media_locator: t.media_locator,
        duration: t.duration_ms ? t.duration_ms / 1000 : undefined,
      }));
      if (metaTracks.length > 0) {
        audioService.playTracks(metaTracks, 0);
      }
    } catch (err) {
      logError("plex.quick-play", err);
    }
  }, []);

  const handleTogglePlexCardFav = useCallback(async (e: React.MouseEvent, album: PlexAlbum) => {
    e.stopPropagation();
    const albumKey = String(album.rating_key || "");
    if (!albumKey) return;

    const favItem: FavoriteAlbum = {
      id: albumKey,
      source: "plex",
      title: album.title,
      artist: album.artist,
      year: album.year != null ? String(album.year) : undefined,
      thumb: null,
      plex_image: album.thumb || null,
      path_or_key: albumKey,
      exists: true,
    };

    try {
      const added = await favoritesService.toggleFavorite(favItem);
      setPlexFavIds((prev) => {
        const next = new Set(prev);
        if (added) next.add(albumKey);
        else next.delete(albumKey);
        return next;
      });
    } catch (err) {
      logError("plex.favorite-toggle", err);
    }
  }, []);

  const handleSelectArtist = useCallback((artistKey: string, artistName: string) => {
    const normalizedArtistKey = artistKey.trim();
    if (!normalizedArtistKey) return;
    setFallbackPlexArtist(null);
    setActiveArtist({ rating_key: normalizedArtistKey, name: artistName });
  }, []);

  const handlePlexModeChange = (mode: "albums" | "artists" | "years" | "genres" | "collections") => {
    setPlexViewMode(mode);
    setActiveAlbum(null);
    setActiveArtist(null);
    setActiveCollection(null);
    setFallbackPlexArtist(null);
    setSelectedPlexDecade(null);
    setSelectedPlexYear(null);
    setSelectedPlexUnknownYear(false);
    setSelectedPlexGenre(null);
  };

  const resetAllNavigation = () => {
    playerNavigationRequests.invalidate();
    setPlayerNavigation(null);
    setPlayerNavigationError(false);
    setExploreDetail(null);
    setLocalSelectedArtist(null);
    setActiveAlbum(null);
    setActiveCollection(null);
    setActiveArtist(null);
    setFallbackPlexArtist(null);
    setSelectedPlexDecade(null);
    setSelectedPlexYear(null);
    setSelectedPlexUnknownYear(false);
    setSelectedPlexGenre(null);
    setSearchQuery("");
    setSearchResults(null);
  };

  const navigateLocalSubview = (view: LocalSubview) => {
    if (playerTarget?.kind === "local") {
      changePlayerLocalSubview(view);
      return;
    }
    setLocalSubview(view);
    if (!playerTarget && centralSection === "local") return;
    resetAllNavigation();
    mainSectionPreference.navigate("local");
  };

  const handlePlexLibraryChange = (libraryKey: string) => {
    const library = libraries.find((item) => item.key === libraryKey);
    if (!library || library.key === selectedLibrary?.key) return;
    plexLibraryRequestGenerationRef.current += 1;
    plexSearchRequestGenerationRef.current += 1;
    setAlbums([]);
    setCollections([]);
    setHasCollections(false);
    setLoading(true);
    setActiveAlbum(null);
    setActiveCollection(null);
    setActiveArtist(null);
    setFallbackPlexArtist(null);
    setSelectedPlexDecade(null);
    setSelectedPlexYear(null);
    setSelectedPlexUnknownYear(false);
    setSelectedPlexGenre(null);
    setSearchQuery("");
    setSearchResults(null);
    setSelectedLibrary(library);
  };

  const isPlexConnected = Boolean(config?.plex_token && config.plex_token.trim().length > 0);
  const localExploreConfigured = Boolean(config?.local_folders.length);
  const localExploreState: ExploreSourceState = !localExploreConfigured ? "not_configured"
    : mpdHealth.state === "starting" ? "loading"
    : !isPlaybackAvailable || playbackStatus.is_updating || localCatalog.saveBlocked || localCatalog.mpdBusy || localCatalog.error ? "unavailable"
    : !localCatalog.loaded ? "loading" : localCatalog.albums.length ? "ready" : "empty";
  const aggregateCurrent = plexAggregate.identity === plexConnectionIdentity;
  const aggregateLoading = aggregateCurrent && plexAggregate.libraries.some(item => item.phase === "loading" || item.phase === "queued");
  const aggregateFailures = aggregateCurrent ? plexAggregate.libraries.filter(item => item.phase === "error").map(item => item.library.title) : [];
  const aggregateAlbums = aggregateCurrent ? plexAggregate.albums : [];
  const plexExploreState: ExploreSourceState = !isPlexConnected ? "not_configured"
    : !aggregateCurrent || plexLibrariesIdentity !== plexConnectionIdentity || loadingLibraries ? "loading"
    : aggregateAlbums.length ? "ready" : plexLibrariesError || aggregateFailures.length ? "unavailable"
    : aggregateLoading || (!plexAggregate.started && libraries.length > 0) ? "loading" : "empty";
  const aggregateComplete = aggregateCurrent && !loadingLibraries && !plexLibrariesError && plexAggregate.started
    && plexAggregate.libraries.length === libraries.length && plexAggregate.libraries.every(item => item.phase === "complete");
  const exploreContext: ExploreContext = {
    local: { albums: localCatalog.albums, state: localExploreState },
    plex: { albums: aggregateAlbums, catalogs: aggregateCurrent ? plexAggregate.catalogs : [], state: plexExploreState,
      identity: plexConnectionIdentity, serverId: config?.plex_server_id ?? "", libraryId: "", loading: aggregateLoading,
      completeness: aggregateComplete ? "complete" : aggregateAlbums.length ? "partial" : "unknown" },
  };
  const exploreLive = useRef({ context: exploreContext, visible: false });
  exploreLive.current = { context: exploreContext, visible: centralSection === "explore" && !playerTarget && !exploreDetail };
  const exploreEntryCurrent = (entry: ExploreEntry) => {
    const current = exploreLive.current.context;
    return exploreEntryIsCurrent(current, entry);
  };
  const openExploreAlbum = (entry: ExploreEntry) => {
    if (!exploreLive.current.visible || !exploreEntryCurrent(entry)) return;
    setExploreLocalSubview("browser");
    setExploreDetail(entry);
  };
  const closeExploreAlbum = () => { if (!document.querySelector("[data-local-tag-dialog-open]")) setExploreDetail(null); };

  const plexLibraryOptions = useMemo(() => libraries.length > 0
    ? libraries.map((library) => ({ value: library.key, label: library.title }))
    : [{
        value: "",
        label: t(loadingLibraries ? "sidebar.loadingLibraries" : "sidebar.noLibraries"),
      }], [libraries, loadingLibraries, t]);
  const plexAlbumSortOptions = useMemo(() => [
    { value: "added_recent", label: t("plex.sortAddedRecent") },
    { value: "album_asc", label: t("plex.sortAlbumAsc") },
    { value: "album_desc", label: t("plex.sortAlbumDesc") },
    { value: "artist_asc", label: t("plex.sortArtistAsc") },
    { value: "artist_desc", label: t("plex.sortArtistDesc") },
    { value: "year_desc", label: t("plex.sortYearNewest") },
    { value: "year_asc", label: t("plex.sortYearOldest") },
  ], [t]);
  const plexArtistSortOptions = useMemo(() => [
    { value: "artist_asc", label: t("plex.sortArtistAsc") },
    { value: "artist_desc", label: t("plex.sortArtistDesc") },
  ], [t]);
  const plexGenreOptions = useMemo(() => deriveAlbumGenres(albums), [albums]);
  const activePlexGenre = plexViewMode === "genres" ? validAlbumGenre(selectedPlexGenre, plexGenreOptions) : null;
  const selectedPlexGenreSummary = plexGenreOptions.find(genre => genre.value === activePlexGenre);
  const selectedPlexGenreAlbums = useMemo(() => activePlexGenre === null ? [] : sortPlexAlbums(filterAlbumsByGenre(albums, activePlexGenre), plexSortMode),
    [albums, activePlexGenre, plexSortMode]);
  useEffect(() => { if (selectedPlexGenre !== activePlexGenre) setSelectedPlexGenre(activePlexGenre); }, [selectedPlexGenre, activePlexGenre]);
  const sortedPlexAlbums = useMemo(
    () => sortPlexAlbums(albums, plexSortMode),
    [albums, plexSortMode],
  );
  const plexAlbumBucketIndices = useMemo(
    () => plexAlbumBucketFirstIndices(sortedPlexAlbums, plexSortMode),
    [plexSortMode, sortedPlexAlbums],
  );
  const plexArtists = useMemo(() => derivePlexArtists(albums), [albums]);
  const sortedPlexArtists = useMemo(
    () => sortPlexArtists(plexArtists, plexArtistSortMode),
    [plexArtistSortMode, plexArtists],
  );
  const plexArtistBucketIndices = useMemo(
    () => plexArtistBucketFirstIndices(sortedPlexArtists),
    [sortedPlexArtists],
  );
  const fallbackPlexArtistAlbums = useMemo(
    () => fallbackPlexArtist
      ? sortPlexAlbums(albumsForPlexArtist(albums, fallbackPlexArtist), "album_asc")
      : [],
    [albums, fallbackPlexArtist],
  );
  const plexYearNavigation = useMemo(() => derivePlexYearNavigation(albums), [albums]);
  const selectedPlexYearAlbums = useMemo(
    () => selectedPlexUnknownYear
      ? plexAlbumsWithoutYear(albums)
      : selectedPlexYear === null
        ? []
        : plexAlbumsForYear(albums, selectedPlexYear),
    [albums, selectedPlexUnknownYear, selectedPlexYear],
  );
  const selectedPlexDecadeSummary = selectedPlexDecade === null
    ? undefined
    : plexYearNavigation.decades.find((item) => item.decade === selectedPlexDecade);
  const hasPlexSubview = Boolean(
    activeCollection
    || fallbackPlexArtist
    || selectedPlexDecade !== null
    || selectedPlexYear !== null
    || selectedPlexUnknownYear
    || activePlexGenre !== null,
  );
  const visibleSearchAlbums = useMemo(() => filterAlbumsByGenre(searchResults?.albums ?? [],
    activePlexGenre), [searchResults, activePlexGenre]);
  const showPlexAlbumIndex = searchQuery.trim().length === 0
    && plexViewMode === "albums"
    && isTextPlexAlbumSort(plexSortMode);
  const showPlexArtistIndex = !hasPlexSubview
    && searchQuery.trim().length === 0
    && plexViewMode === "artists";

  const handlePlexContextBack = () => {
    if (activeCollection) {
      setActiveCollection(null);
    } else if (fallbackPlexArtist) {
      setFallbackPlexArtist(null);
    } else if (activePlexGenre !== null) {
      setSelectedPlexGenre(null);
    } else if (selectedPlexUnknownYear) {
      setSelectedPlexUnknownYear(false);
    } else if (selectedPlexYear !== null) {
      setSelectedPlexYear(null);
    } else if (selectedPlexDecade !== null) {
      setSelectedPlexDecade(null);
    }
  };

  if (!config || mediaSource === null) {
    return (
      <main className="h-screen w-screen flex items-center justify-center bg-[#121212] text-[#E0E0E0]">
        <div className="max-w-xl px-6 text-center">
          <h1 className="text-xl font-black tracking-wider text-[#E5A00D]">SONANTE</h1>
          <p className="mt-3 text-sm text-[#888888]">
            {t(managedSavePending ? "app.managedSavePending" : bootstrapError ? "app.bootstrapError" : "wizard.starting")}
          </p>
        </div>
      </main>
    );
  }

  if (config && config.first_run) {
    return (
      <WelcomeWizard
        initialConfig={config}
        devices={devices}
        onFinish={(newConfig) => {
          setConfig(newConfig);
          mainSectionPreference.finishSetup(newConfig);
        }}
      />
    );
  }

  return (
    <div className="h-screen w-screen flex flex-col bg-[#121212] text-[#E0E0E0] overflow-hidden select-none"
      onClickCapture={() => playerNavigationRequests.invalidate()}
      onChangeCapture={() => playerNavigationRequests.invalidate()}>
      <div className="min-h-0 flex-1 flex overflow-hidden">
        {/* Barra Lateral */}
        <aside className="flex min-h-0 w-56 shrink-0 flex-col overflow-hidden border-r border-[#262626] bg-[#181818] p-3">
          <div className="mb-7 flex shrink-0 items-center gap-2.5 px-3 py-3">
            <Sparkles className="shrink-0 text-[#E5A00D]" size={21} />
            <h1 className="truncate text-base font-black tracking-[0.14em] text-[#E5A00D]">SONANTE</h1>
          </div>

          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto pb-3">
            <section aria-labelledby="sidebar-music-group">
              <h2 id="sidebar-music-group" className="mb-2 px-3 text-[10px] font-bold uppercase tracking-[0.16em] text-[#5F5F5F]">
                {t("sidebar.musicGroup")}
              </h2>
              <nav aria-label={t("sidebar.musicGroup")} className="space-y-1">
                <SidebarNavItem icon={Compass} label={t("explore.title")} active={mediaSource === "explore"}
                  onClick={() => { resetAllNavigation(); mainSectionPreference.navigate("explore"); }} />
                <SidebarNavItem
                  icon={Folder}
                  label={t("sidebar.local")}
                  active={mediaSource === "local"}
                  onClick={() => navigateLocalSubview("browser")}
                  trailing={!isPlaybackAvailable ? (
                    <span
                      className="flex items-center text-[#C9A45D]"
                      title={mpdHealth.state === "unavailable" ? t("player.engineUnavailable") : t("player.engineTransitioning")}
                    >
                      <CircleAlert size={13} />
                    </span>
                  ) : localCatalogActivity(playbackStatus.is_updating, localCatalog) ? (
                    <span className="flex items-center text-[#E5A00D]" title={t(playbackStatus.is_updating ? "sidebar.indexingTooltip" : "localBrowser.organizing")}>
                      <RefreshCw size={13} className="animate-spin" />
                    </span>
                  ) : (
                    <span className="flex items-center text-[#4BB543]/80" title={t("sidebar.syncedTooltip")}>
                      <CheckCircle2 size={13} />
                    </span>
                  )}
                />
                <SidebarNavItem
                  icon={Server}
                  label={t("sidebar.plex")}
                  active={mediaSource === "plex"}
                  onClick={() => {
                    resetAllNavigation();
                    mainSectionPreference.navigate("plex");
                  }}
                />
              </nav>
            </section>

            <section aria-labelledby="sidebar-collection-group">
              <h2 id="sidebar-collection-group" className="mb-2 px-3 text-[10px] font-bold uppercase tracking-[0.16em] text-[#5F5F5F]">
                {t("sidebar.collectionGroup")}
              </h2>
              <nav aria-label={t("sidebar.collectionGroup")} className="space-y-1">
                <SidebarNavItem
                  icon={Heart}
                  label={t("sidebar.favorites")}
                  active={mediaSource === "favorites"}
                  onClick={() => {
                    resetAllNavigation();
                    mainSectionPreference.navigate("favorites");
                  }}
                />
                <SidebarNavItem
                  icon={ListMusic}
                  label={t("sidebar.playlists")}
                  active={mediaSource === "playlists"}
                  onClick={() => {
                    resetAllNavigation();
                    mainSectionPreference.navigate("playlists");
                  }}
                />
              </nav>
            </section>
          </div>

          <nav aria-label={t("sidebar.utilitiesGroup")} className="mt-auto shrink-0 space-y-1 pt-4">
            <SidebarNavItem
              icon={Settings}
              label={t("sidebar.preferences")}
              onClick={() => { if (!configSaveInProgressRef.current) setShowSettings(true); }}
            />
            <SidebarNavItem
              icon={Info}
              label={t("sidebar.about")}
              onClick={() => setShowAbout(true)}
            />
          </nav>
        </aside>

        {/* Painel Central */}
        <div className={playerTarget ? "hidden" : "contents"}>
        {centralSection === "explore" ? (
          <div className="contents">
            <div className={exploreDetail ? "hidden" : "contents"}>
              <ExploreView portraits={artistPortraits} context={exploreContext} active={!exploreDetail && !playerTarget && !showCollectionWizard} libraryTitle={null}
                configurationReady={config !== null} onConfigure={() => setShowCollectionWizard(true)}
                canRetryLocal={isPlaybackAvailable && !playbackStatus.is_updating && !localCatalog.saveBlocked && !localCatalog.loading && !localCatalog.mpdBusy}
                retryLocal={() => { if (isPlaybackAvailable && !playbackStatus.is_updating && !localCatalog.saveBlocked && !localCatalog.loading && !localCatalog.mpdBusy) localAlbumCatalog.requestRefresh(); }}
                onlineArtworkEnabled={config?.online_artwork_enabled ?? false}
                artworkBlocked={!isPlaybackAvailable || artworkReadBlocked(playbackStatus.is_updating, localCatalog)}
                plexProgress={{ loading: isPlexConnected && (loadingLibraries || aggregateLoading), failedLibraries: aggregateFailures,
                  discoveryFailed: plexLibrariesError, complete: aggregateComplete }}
                retryPlex={() => { if (plexLibrariesError) setPlexDiscoveryAttempt(attempt => attempt + 1); else plexCatalog.retryFailures(); }}
                ensureLocal={() => localAlbumCatalog.ensureLoaded(playbackStatus.is_updating, localExploreConfigured && isPlaybackAvailable)}
                isCurrent={exploreEntryCurrent} onOpen={openExploreAlbum} />
            </div>
            {exploreDetail?.source === "local" && <LocalBrowserView initialAlbumId={exploreDetail.album.id}
              subview={exploreLocalSubview} onSubviewChange={setExploreLocalSubview}
              albumNavigationAvailable={exploreEntryCurrent(exploreDetail)} onAlbumNavigationBack={closeExploreAlbum}
              catalogRefreshRevision={localCatalogRevision} isPlaybackAvailable={isPlaybackAvailable} isLibraryUpdating={playbackStatus.is_updating}
              onlineArtworkEnabled={config?.online_artwork_enabled ?? false} configuredLocalFolders={config?.local_folders ?? []} />}
            {exploreDetail?.source === "plex" && (exploreEntryCurrent(exploreDetail)
              ? <AlbumView key={exploreDetail.key} album={exploreDetail.album} onBack={closeExploreAlbum}
                  onToggleFavorite={refreshPlexFavorites} isPlaybackAvailable={isPlaybackAvailable} />
              : <main className="flex-1 space-y-3 p-6 text-sm text-[#BBBBBB]"><p role="status">{t("explore.detailUnavailable")}</p>
                  <button type="button" onClick={closeExploreAlbum}>{t("explore.back")}</button></main>)}
          </div>
        ) : centralSection === "playlists" ? (
          <PlaylistsView isPlaybackAvailable={isPlaybackAvailable} />
        ) : centralSection === "favorites" ? (
          <FavoritesView
            onFavoritesChanged={refreshPlexFavorites}
            isPlaybackAvailable={isPlaybackAvailable}
          />
        ) : centralSection === "local" ? (
          <LocalBrowserView
            subview={localSubview} onSubviewChange={setLocalSubview}
            initialArtist={localSelectedArtist}
            onClearInitialArtist={() => setLocalSelectedArtist(null)}
            importPreview={importPreview}
            catalogRefreshRevision={localCatalogRevision}
            isPlaybackAvailable={isPlaybackAvailable}
            isLibraryUpdating={playbackStatus.is_updating}
            onlineArtworkEnabled={config?.online_artwork_enabled ?? false}
            configuredLocalFolders={config?.local_folders ?? []}
          />
        ) : !isPlexConnected ? (
          <main className="flex-1 flex flex-col items-center justify-center bg-[#121212] select-none p-8 text-center animate-in fade-in duration-200">
            <div className="w-16 h-16 rounded-2xl bg-[#E5A00D]/10 border border-[#E5A00D]/20 flex items-center justify-center text-[#E5A00D] mb-4 shadow-xl">
              <Server size={32} />
            </div>
            <h2 className="text-xl font-bold text-white mb-2">{t("plex.disconnectedTitle")}</h2>
            <p className="text-xs text-[#888888] max-w-md mb-6 leading-relaxed">
              {t("plex.disconnectedDesc")}
            </p>
            <button
              onClick={() => { if (!configSaveInProgressRef.current) setShowSettings(true); }}
              className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer"
            >
              <ExternalLink size={15} />
              <span>{t("plex.connectBtn")}</span>
            </button>
          </main>
	  ) : activeAlbum ? (
          <AlbumView
            album={activeAlbum}
            onBack={() => setActiveAlbum(null)}
            onSelectArtist={(art) => {
              const artistKey = art.rating_key.trim();
              if (!artistKey) return;
              setActiveAlbum(null);
              setActiveArtist({ ...art, rating_key: artistKey });
            }}
            onToggleFavorite={refreshPlexFavorites}
            isPlaybackAvailable={isPlaybackAvailable}
          />
        ) : activeArtist ? (
          <ArtistView
            artist={activeArtist}
            onBack={() => setActiveArtist(null)}
            onSelectAlbum={(alb) => setActiveAlbum(alb)}
            status={playbackStatus}
            isPlaybackAvailable={isPlaybackAvailable}
          />
        ) : (
          <main className="flex-1 flex flex-col overflow-hidden bg-[#121212]">
            <div className="flex flex-wrap items-center justify-between gap-4 px-8 pt-8 pb-4">
              <div className="flex items-center space-x-3">
                {hasPlexSubview && (
                  <button
                    onClick={handlePlexContextBack}
                    className="p-1.5 rounded-lg bg-[#1E1E1E] border border-[#333333] hover:bg-[#2A2A2A] text-white transition-colors cursor-pointer mr-1"
                    title={t("plex.back")}
                  >
                    <ArrowLeft size={16} />
                  </button>
                )}
                <div>
                  <h2 className="text-2xl font-bold text-white tracking-tight">
                    {searchQuery.trim().length > 0
                      ? t("plex.resultsFor", { query: searchQuery })
                      : activeCollection
                        ? activeCollection.title
                        : fallbackPlexArtist
                          ? fallbackPlexArtist.name
                          : selectedPlexGenreSummary
                            ? selectedPlexGenreSummary.label
                          : selectedPlexUnknownYear
                            ? t("plex.unknownYear")
                            : selectedPlexYear !== null
                              ? String(selectedPlexYear)
                              : selectedPlexDecade !== null
                                ? `${selectedPlexDecade}s`
                                : plexViewMode === "genres" ? t("common.genresLabel") : t("player.source.plex")}
                  </h2>
                  {searchQuery.trim().length === 0 && (
                    <p className="mt-0.5 text-xs text-[#888888]">
                      {activeCollection
                        ? t("plex.albumsCount", { count: collectionAlbums.length })
                        : fallbackPlexArtist
                          ? t("plex.albumsCount", { count: fallbackPlexArtistAlbums.length })
                          : activePlexGenre !== null
                            ? t("plex.albumsCount", { count: selectedPlexGenreAlbums.length })
                          : selectedPlexUnknownYear || selectedPlexYear !== null
                            ? t("plex.albumsCount", { count: selectedPlexYearAlbums.length })
                            : selectedPlexDecadeSummary
                              ? t("plex.albumsCount", { count: selectedPlexDecadeSummary.albumCount })
                        : plexViewMode === "collections"
                          ? t("plex.collectionsCount", { count: collections.length })
                          : plexViewMode === "artists"
                            ? t("plex.artistsCount", { count: plexArtists.length })
                          : t("plex.albumsCount", { count: albums.length })}
                    </p>
                  )}
                </div>
              </div>
            </div>

            {!hasPlexSubview && searchQuery.trim().length === 0 && (
              <div className="flex flex-wrap gap-1 border-b border-[#222222] px-8 pb-3">
                <button
                  type="button"
                  onClick={() => handlePlexModeChange("albums")}
                  className={`rounded-md px-4 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                    plexViewMode === "albums" ? "bg-[#E5A00D] text-black shadow" : "text-[#999999] hover:bg-[#1E1E1E] hover:text-white"
                  }`}
                >
                  {t("plex.tabAlbums")}
                </button>
                <button
                  type="button"
                  onClick={() => handlePlexModeChange("artists")}
                  className={`rounded-md px-4 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                    plexViewMode === "artists" ? "bg-[#E5A00D] text-black shadow" : "text-[#999999] hover:bg-[#1E1E1E] hover:text-white"
                  }`}
                >
                  {t("plex.tabArtists")}
                </button>
                <button
                  type="button"
                  onClick={() => handlePlexModeChange("years")}
                  className={`rounded-md px-4 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                    plexViewMode === "years" ? "bg-[#E5A00D] text-black shadow" : "text-[#999999] hover:bg-[#1E1E1E] hover:text-white"
                  }`}
                >
                  {t("plex.tabYears")}
                </button>
                <button
                  type="button"
                  onClick={() => handlePlexModeChange("genres")}
                  className={`rounded-md px-4 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                    plexViewMode === "genres" ? "bg-[#E5A00D] text-black shadow" : "text-[#999999] hover:bg-[#1E1E1E] hover:text-white"
                  }`}
                >
                  {t("common.genresLabel")}
                </button>
                {hasCollections && (
                  <button
                    type="button"
                    onClick={() => handlePlexModeChange("collections")}
                    className={`rounded-md px-4 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                      plexViewMode === "collections" ? "bg-[#E5A00D] text-black shadow" : "text-[#999999] hover:bg-[#1E1E1E] hover:text-white"
                    }`}
                  >
                    {t("plex.tabCollections")}
                  </button>
                )}
              </div>
            )}

            <div className="flex flex-wrap items-center gap-3 border-b border-[#222222] px-8 py-3">
              <div className="flex items-center gap-2 text-xs text-[#888888]">
                <span>{t("plex.libraryLabel")}</span>
                <ToolbarSelect
                  value={selectedLibrary?.key ?? ""}
                  options={plexLibraryOptions}
                  onChange={handlePlexLibraryChange}
                  label={t("plex.libraryLabel")}
                  disabled={loadingLibraries || libraries.length === 0}
                  className="max-w-52"
                />
              </div>

              <div className="relative flex w-72 max-w-full items-center">
                <Search size={15} className="absolute left-3 text-[#666666]" />
                <input
                  ref={searchInputRef}
                  type="text"
                  value={searchQuery}
                  onChange={(event) => setSearchQuery(event.target.value)}
                  placeholder={t("plex.searchPlaceholder")}
                  className="w-full rounded-lg border border-[#2B2B2B] bg-[#1A1A1A] py-1.5 pl-9 pr-8 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D]"
                />
                {searchQuery && (
                  <button type="button" onClick={() => setSearchQuery("")} className="absolute right-2.5 text-[#666666] hover:text-white cursor-pointer">
                    <X size={14} />
                  </button>
                )}
              </div>

              {searchQuery.trim().length === 0 && ((!hasPlexSubview && plexViewMode === "albums") || activePlexGenre !== null) && (
                <div className="flex items-center gap-2 text-xs text-[#888888]">
                  <span>{t("plex.sortLabel")}</span>
                  <ToolbarSelect
                    value={plexSortMode}
                    options={plexAlbumSortOptions}
                    onChange={(nextValue) => setPlexSortMode(nextValue as PlexAlbumSortMode)}
                    label={t("plex.sortLabel")}
                    className="max-w-52"
                  />
                </div>
              )}
              {!hasPlexSubview && searchQuery.trim().length === 0 && plexViewMode === "artists" && (
                <div className="flex items-center gap-2 text-xs text-[#888888]">
                  <span>{t("plex.sortLabel")}</span>
                  <ToolbarSelect
                    value={plexArtistSortMode}
                    options={plexArtistSortOptions}
                    onChange={(nextValue) => setPlexArtistSortMode(nextValue as PlexArtistSortMode)}
                    label={t("plex.sortLabel")}
                    className="max-w-52"
                  />
                </div>
              )}
            </div>

            {(showPlexAlbumIndex || showPlexArtistIndex) && (
              <nav
                aria-label={t(plexViewMode === "artists" ? "plex.artistIndex" : "plex.albumIndex")}
                className="flex flex-wrap items-center gap-1 border-b border-[#222222] px-8 py-2"
              >
                {LOCAL_ALBUM_INDEX_BUCKETS.map((bucket) => {
                  const index = plexViewMode === "artists"
                    ? plexArtistBucketIndices.get(bucket)
                    : plexAlbumBucketIndices.get(bucket);
                  const label = t("plex.jumpToBucket", { bucket });
                  return (
                    <button
                      key={bucket}
                      type="button"
                      disabled={index === undefined}
                      title={label}
                      aria-label={label}
                      onClick={() => {
                        if (index === undefined) return;
                        if (plexViewMode === "artists") plexArtistListRef.current?.scrollToIndex(index);
                        else plexGridRef.current?.scrollToIndex(index);
                      }}
                      className="h-6 min-w-6 rounded px-1 text-[11px] font-semibold text-[#999999] transition-colors hover:bg-[#282828] hover:text-[#E5A00D] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:cursor-default disabled:text-[#444444] disabled:hover:bg-transparent"
                    >
                      {bucket}
                    </button>
                  );
                })}
              </nav>
            )}

            <div ref={setPlexScrollContainer} className="flex-1 overflow-y-auto p-8">
              {searchQuery.trim().length > 0 ? (
                isSearching ? (
                  <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                    {t("plex.searching")}
                  </div>
                ) : searchResults &&
                  (searchResults.artists.length > 0 ||
                    visibleSearchAlbums.length > 0 ||
                    searchResults.tracks.length > 0) ? (
                  <div className="space-y-8">
                    {searchResults.artists.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          {t("plex.artists")}
                        </h3>
                        <div className="flex flex-wrap gap-3">
                          {searchResults.artists.map((art) => (
                            <button
                              key={art.rating_key}
                              onClick={() =>
                                setActiveArtist({ rating_key: art.rating_key, name: art.name })
                              }
                              className="flex items-center space-x-3 bg-[#181818] border border-[#262626] hover:border-[#E5A00D] rounded-full pl-1.5 pr-4 py-1.5 transition-colors cursor-pointer"
                            >
                              <div className="w-8 h-8 rounded-full bg-[#242424] overflow-hidden flex items-center justify-center">
                                {art.thumb ? (
                                  <PlexImage image={art.thumb} alt="" className="w-full h-full object-cover" />
                                ) : (
                                  <User size={14} className="text-[#666666]" />
                                )}
                              </div>
                              <span className="text-xs font-bold text-white">{art.name}</span>
                            </button>
                          ))}
                        </div>
                      </div>
                    )}

                    {visibleSearchAlbums.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          {t("plex.albums")}
                        </h3>
                        <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                          {visibleSearchAlbums.map((album) => {
                            const albumKey = String(album.rating_key || "");
                            return (
                              <PlexAlbumCard
                                key={albumKey || album.title}
                                album={album}
                                isFav={albumKey ? plexFavIds.has(albumKey) : false}
                                onSelect={(a) => setActiveAlbum(a)}
                                onPlayQuick={handlePlayQuick}
                                onToggleFav={handleTogglePlexCardFav}
                                onSelectArtist={handleSelectArtist}
                                removeFavText={t("favorites.removeFavorite")}
                                addFavText={t("common.addFavorite")}
                                playAlbumText={t("plex.playAlbum")}
                                isPlaybackAvailable={isPlaybackAvailable}
                              />
                            );
                          })}
                        </div>
                      </div>
                    )}

                    {searchResults.tracks.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          {t("plex.tracks")}
                        </h3>
                        <div className="divide-y divide-[#1A1A1A] bg-[#141414] rounded-xl border border-[#222222] p-2">
                          {searchResults.tracks.map((track) => (
                            <div
                              key={track.rating_key}
                              onClick={() => {
                                if (!isPlaybackAvailable) return;
                                const meta = [
                                  {
                                    title: track.title,
                                    artist: track.album_title || "Plex Track",
                                    album: track.album_title || "",
                                    plex_image: track.thumb || null,
                                    media_locator: track.media_locator,
                                    duration: track.duration_ms ? track.duration_ms / 1000 : undefined,
                                  },
                                ];
                                audioService.playTracks(meta, 0);
                              }}
                              aria-disabled={!isPlaybackAvailable}
                              className={`flex items-center justify-between p-2.5 rounded-lg transition-colors group ${
                                isPlaybackAvailable
                                  ? "hover:bg-[#1E1E1E] cursor-pointer"
                                  : "opacity-60 cursor-not-allowed"
                              }`}
                            >
                              <div className="flex items-center space-x-3 min-w-0 pr-4">
                                <div className="w-9 h-9 rounded bg-[#202020] overflow-hidden shrink-0">
                                  {track.thumb ? (
                                    <PlexImage image={track.thumb} alt="" className="w-full h-full object-cover" />
                                  ) : (
                                    <div className="w-full h-full flex items-center justify-center text-[#444444]">
                                      <Disc3 size={16} />
                                    </div>
                                  )}
                                </div>
                                <div className="flex flex-col min-w-0">
                                  <span className="text-xs font-bold text-white group-hover:text-[#E5A00D] transition-colors truncate">
                                    {track.title}
                                  </span>
                                  {track.album_title && (
                                    <span className="text-[11px] text-[#777777] truncate">
                                      {track.album_title}
                                    </span>
                                  )}
                                </div>
                              </div>
                              <div className="flex items-center gap-1">
                                <button
                                  type="button"
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    setPlaylistItems([
                                      {
                                        media_locator: track.media_locator,
                                        metadata: {
                                          title: track.title,
                                          artist: track.artist || "",
                                          album: track.album_title || "",
                                          duration: track.duration_ms
                                            ? track.duration_ms / 1000
                                            : track.duration,
                                        },
                                      },
                                    ]);
                                  }}
                                  className="rounded-md p-1.5 text-[#777777] hover:bg-[#2A2A2A] hover:text-[#E5A00D]"
                                  title={t("playlists.addTrack")}
                                >
                                  <ListPlus size={15} />
                                </button>
                                <Play size={14} className="text-[#888888] group-hover:text-white shrink-0 mr-2" />
                              </div>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                ) : (
                  <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                    {t("plex.noResults", { query: searchQuery })}
                  </div>
                )
              ) : loading ? (
                <div className="h-full flex items-center justify-center text-[#666666]">
                  {t("plex.loading")}
                </div>
              ) : activeCollection ? (
                <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                  {collectionAlbums.map((album) => {
                    const albumKey = String(album.rating_key || "");
                    return (
                      <PlexAlbumCard
                        key={albumKey || album.title}
                        album={album}
                        isFav={albumKey ? plexFavIds.has(albumKey) : false}
                        onSelect={(a) => setActiveAlbum(a)}
                        onPlayQuick={handlePlayQuick}
                        onToggleFav={handleTogglePlexCardFav}
                        onSelectArtist={handleSelectArtist}
                        removeFavText={t("favorites.removeFavorite")}
                        addFavText={t("common.addFavorite")}
                        playAlbumText={t("plex.playAlbum")}
                        isPlaybackAvailable={isPlaybackAvailable}
                      />
                    );
                  })}
                </div>
              ) : fallbackPlexArtist ? (
                <VirtualAlbumGrid
                  items={fallbackPlexArtistAlbums}
                  getItemKey={(album) => album.rating_key}
                  scrollContainer={plexScrollContainer}
                  resetKey={JSON.stringify([selectedLibrary?.key, "fallback-artist", fallbackPlexArtist.id])}
                  renderItem={(album) => {
                    const albumKey = String(album.rating_key || "");
                    return (
                      <PlexAlbumCard
                        album={album}
                        isFav={albumKey ? plexFavIds.has(albumKey) : false}
                        onSelect={setActiveAlbum}
                        onPlayQuick={handlePlayQuick}
                        onToggleFav={handleTogglePlexCardFav}
                        onSelectArtist={handleSelectArtist}
                        removeFavText={t("favorites.removeFavorite")}
                        addFavText={t("common.addFavorite")}
                        playAlbumText={t("plex.playAlbum")}
                        isPlaybackAvailable={isPlaybackAvailable}
                      />
                    );
                  }}
                />
              ) : selectedPlexYear !== null || selectedPlexUnknownYear ? (
                <VirtualAlbumGrid
                  items={selectedPlexYearAlbums}
                  getItemKey={(album) => album.rating_key}
                  scrollContainer={plexScrollContainer}
                  resetKey={JSON.stringify([selectedLibrary?.key, selectedPlexYear, selectedPlexUnknownYear])}
                  renderItem={(album) => {
                    const albumKey = String(album.rating_key || "");
                    return (
                      <PlexAlbumCard
                        album={album}
                        isFav={albumKey ? plexFavIds.has(albumKey) : false}
                        onSelect={setActiveAlbum}
                        onPlayQuick={handlePlayQuick}
                        onToggleFav={handleTogglePlexCardFav}
                        onSelectArtist={handleSelectArtist}
                        removeFavText={t("favorites.removeFavorite")}
                        addFavText={t("common.addFavorite")}
                        playAlbumText={t("plex.playAlbum")}
                        isPlaybackAvailable={isPlaybackAvailable}
                      />
                    );
                  }}
                />
              ) : plexViewMode === "genres" ? (
                activePlexGenre !== null ? (
                  <VirtualAlbumGrid
                    items={selectedPlexGenreAlbums}
                    getItemKey={(album) => album.rating_key}
                    scrollContainer={plexScrollContainer}
                    resetKey={JSON.stringify([selectedLibrary?.key, "genre", activePlexGenre, plexSortMode])}
                    renderItem={(album) => {
                      const albumKey = String(album.rating_key || "");
                      return (
                        <PlexAlbumCard
                          album={album}
                          isFav={albumKey ? plexFavIds.has(albumKey) : false}
                          onSelect={setActiveAlbum}
                          onPlayQuick={handlePlayQuick}
                          onToggleFav={handleTogglePlexCardFav}
                          onSelectArtist={handleSelectArtist}
                          removeFavText={t("favorites.removeFavorite")}
                          addFavText={t("common.addFavorite")}
                          playAlbumText={t("plex.playAlbum")}
                          isPlaybackAvailable={isPlaybackAvailable}
                        />
                      );
                    }}
                  />
                ) : <AlbumGenreGrid genres={plexGenreOptions} onSelect={setSelectedPlexGenre}
                  albumCountLabel={count => t("plex.albumsCount", { count })} />
              ) : plexViewMode === "albums" ? (
                <VirtualAlbumGrid
                  ref={plexGridRef}
                  items={sortedPlexAlbums}
                  getItemKey={(album) => album.rating_key}
                  scrollContainer={plexScrollContainer}
                  resetKey={JSON.stringify([plexAlbumNavigationResetKey(selectedLibrary?.key ?? null, plexSortMode)])}
                  renderItem={(album) => {
                    const albumKey = String(album.rating_key || "");
                    return (
                      <PlexAlbumCard
                        album={album}
                        isFav={albumKey ? plexFavIds.has(albumKey) : false}
                        onSelect={(a) => setActiveAlbum(a)}
                        onPlayQuick={handlePlayQuick}
                        onToggleFav={handleTogglePlexCardFav}
                        onSelectArtist={handleSelectArtist}
                        removeFavText={t("favorites.removeFavorite")}
                        addFavText={t("common.addFavorite")}
                        playAlbumText={t("plex.playAlbum")}
                        isPlaybackAvailable={isPlaybackAvailable}
                      />
                    );
                  }}
                />
              ) : plexViewMode === "artists" ? (
                <VirtualList
                  ref={plexArtistListRef}
                  items={sortedPlexArtists}
                  rowHeight={56}
                  getItemKey={(artist) => artist.id}
                  scrollContainer={plexScrollContainer}
                  resetKey={JSON.stringify([selectedLibrary?.key, plexArtistSortMode])}
                  renderItem={(artist) => (
                    <button
                      type="button"
                      onClick={() => {
                        if (artist.ratingKey) {
                          setActiveArtist({ rating_key: artist.ratingKey, name: artist.name });
                        } else {
                          setFallbackPlexArtist(artist);
                        }
                      }}
                      className="flex h-full w-full items-center justify-between border-b border-[#1D1D1D] px-4 text-left transition-colors hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[#E5A00D]"
                    >
                      <span className="truncate pr-4 text-sm font-semibold text-white">{artist.name}</span>
                      <span className="shrink-0 text-xs text-[#777777]">
                        {t("plex.albumsCount", { count: artist.albumCount })}
                      </span>
                    </button>
                  )}
                />
              ) : plexViewMode === "years" ? (
                selectedPlexDecadeSummary ? (
                  <div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-4">
                    {selectedPlexDecadeSummary.years.map((year) => (
                      <button
                        key={year.year}
                        type="button"
                        onClick={() => setSelectedPlexYear(year.year)}
                        className="rounded-xl border border-[#2B2B2B] bg-[#181818] p-5 text-left transition-colors hover:border-[#E5A00D] hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
                      >
                        <span className="block text-xl font-bold text-white">{year.year}</span>
                        <span className="mt-1 block text-xs text-[#777777]">
                          {t("plex.albumsCount", { count: year.albumCount })}
                        </span>
                      </button>
                    ))}
                  </div>
                ) : (
                  <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-4">
                    {plexYearNavigation.decades.map((decade) => (
                      <button
                        key={decade.decade}
                        type="button"
                        onClick={() => setSelectedPlexDecade(decade.decade)}
                        aria-label={t("plex.decadeLabel", { decade: decade.decade })}
                        className="rounded-xl border border-[#2B2B2B] bg-[#181818] p-5 text-left transition-colors hover:border-[#E5A00D] hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
                      >
                        <span className="block text-xl font-bold text-white">{t("plex.decadeShort", { decade: decade.decade })}</span>
                        <span className="mt-1 block text-xs text-[#777777]">
                          {t("plex.albumsCount", { count: decade.albumCount })}
                        </span>
                      </button>
                    ))}
                    {plexYearNavigation.unknownCount > 0 && (
                      <button
                        type="button"
                        onClick={() => setSelectedPlexUnknownYear(true)}
                        className="rounded-xl border border-[#2B2B2B] bg-[#181818] p-5 text-left transition-colors hover:border-[#E5A00D] hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
                      >
                        <span className="block text-lg font-bold text-white">{t("plex.unknownYear")}</span>
                        <span className="mt-1 block text-xs text-[#777777]">
                          {t("plex.albumsCount", { count: plexYearNavigation.unknownCount })}
                        </span>
                      </button>
                    )}
                  </div>
                )
              ) : (
                <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                  {collections.map((col) => (
                    <div
                      key={col.rating_key}
                      onClick={() => handleSelectCollection(col)}
                      className="flex flex-col cursor-pointer group"
                    >
                      <PlexCollectionArtwork collection={col} />

                      <span className="text-sm font-semibold text-white truncate" title={col.title}>
                        {col.title}
                      </span>
                      <span className="text-xs text-[#E5A00D] font-bold mt-0.5">
                        {t("plex.itemsCount", { count: col.child_count })}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </main>
        )}
        </div>
        {playerTarget && (
          <div className="contents">
            {playerTarget.kind === "local" ? (
              <LocalBrowserView
                key={`${playerNavigation?.revision}:${playerTarget.view}:${playerTarget.albumId}`}
                subview={playerSubview} onSubviewChange={changePlayerLocalSubview}
                playerTarget={playerTarget}
                onPlayerNavigationBack={closePlayerNavigation}
                catalogRefreshRevision={localCatalogRevision}
                isPlaybackAvailable={isPlaybackAvailable}
                isLibraryUpdating={playbackStatus.is_updating}
                onlineArtworkEnabled={config?.online_artwork_enabled ?? false}
                configuredLocalFolders={config?.local_folders ?? []}
              />
            ) : playerTarget.view === "album" ? (
              <AlbumView
                key={`${playerNavigation?.revision}:${playerTarget.album.rating_key}`}
                album={playerTarget.album}
                onBack={() => playerTarget.returnToArtist
                  ? setPlayerTarget({ ...playerTarget, view: "artist", returnToArtist: false })
                  : closePlayerNavigation()}
                onSelectArtist={artist => setPlayerTarget({ ...playerTarget, artist, view: "artist", returnToArtist: false })}
                onToggleFavorite={refreshPlexFavorites}
                isPlaybackAvailable={isPlaybackAvailable}
              />
            ) : (
              <ArtistView
                key={`${playerNavigation?.revision}:${playerTarget.artist.rating_key}`}
                artist={playerTarget.artist}
                onBack={closePlayerNavigation}
                onSelectAlbum={album => setPlayerTarget({ ...playerTarget, album, view: "album", returnToArtist: true })}
                status={playbackStatus}
                isPlaybackAvailable={isPlaybackAvailable}
              />
            )}
          </div>
        )}
      </div>

      {playerNavigationError && (
        <div role="alert" className="flex items-center justify-between gap-4 border-t border-[#383838] bg-[#211D14] px-6 py-2 text-xs text-[#D7AD52]">
          <span>{t("player.navigationUnavailable")}</span>
          <button type="button" onClick={() => setPlayerNavigationError(false)} aria-label={t("player.dismissNavigationError")} className="p-1"><X size={14} /></button>
        </div>
      )}

      <PlayerBar
        route={configuredSignalRoute(config)}
        status={playbackStatus}
        health={mpdHealth}
        onOpenNowPlaying={() => {
          openNowPlayingWindow(t("nowPlaying.windowTitle")).catch((err) => {
            logError("now-playing.open", err);
          });
        }}
        onToggleQueue={() => setShowQueue(!showQueue)}
        isQueueOpen={showQueue}
        onNavigateToArtist={() => navigateFromPlayer("artist")}
        onNavigateToAlbum={() => navigateFromPlayer("album")}
      />
      {(configSavePhase || configSaveNotice) && (
        <div
          role={configSaveNotice?.kind === "error" ? "alert" : "status"}
          aria-live={configSaveNotice?.kind === "error" ? "assertive" : "polite"}
          className="fixed bottom-28 right-6 z-40 max-h-[60vh] max-w-sm overflow-y-auto rounded-lg border border-[#383838] bg-[#1D1D1D] p-4 text-xs text-white shadow-xl"
        >
          <div className="flex items-start gap-2">
            {configSavePhase ? (
              <RefreshCw size={16} className="shrink-0 animate-spin text-[#E5A00D]" />
            ) : configSaveNotice?.kind === "success" ? (
              <CheckCircle2 size={16} className="shrink-0 text-[#4BB543]" />
            ) : (
              <CircleAlert size={16} className="shrink-0 text-red-400" />
            )}
            <div className="min-w-0 flex-1">
              <div className="font-semibold">
                {configSavePhase === "local" ? t(localSavePhase === "preparing" ? "settings.preparingLibrary"
                  : localSavePhase === "validating" ? "settings.validatingLibrary" : "settings.updatingLibrary")
                  : configSavePhase === "other" ? t("settings.savingSettings")
                  : configSaveNotice?.kind === "success" ? t("settings.libraryUpdateSucceeded")
                  : configSaveNotice?.uncertain ? t("settings.librarySaveUncertain")
                  : configSaveNotice?.refusal === "fullRescan" ? t("settings.fullIndexRequired")
                  : configSaveNotice?.refusal === "planChanged" ? t("settings.libraryChangedBeforeSave")
                  : configSaveNotice?.refusal === "queueChanged" ? t("settings.queueClearStale")
                  : configSaveNotice?.refusal === "queueCheckRequired" ? t("settings.queueClearCheckRequired")
                  : t("settings.libraryUpdateFailed")}
              </div>
              {configSavePhase === "local" && <div className="mt-1 text-[#999999]">{t("settings.indexingMayTakeTime")}</div>}
              {configSaveNotice?.kind === "error" && (
                <div className="mt-1 whitespace-pre-wrap break-words text-red-300">
                  {configSaveNotice.refusal && <div>{t("settings.noChangesApplied")}</div>}
                  {configSaveNotice.refusal === "fullRescan" && <div className="mt-1">{t("settings.fullIndexNotStarted")}</div>}
                  {(configSaveNotice.refusal === "queueChanged" || configSaveNotice.refusal === "queueCheckRequired") && <div className="mt-1">{t("settings.queueClearRecheck")}</div>}
                  {configSaveNotice.detail}
                  {configSaveNotice.statusUnavailable && <div>{t("settings.librarySaveStatusUnavailable")}</div>}
                  {configSaveNotice.stagingCleanupFailed && <div className="mt-1">{t("settings.stagingCleanupFailed")}</div>}
                  {configSaveNotice.reloadFailed && <div className="mt-1">{t("settings.confirmedConfigUnavailable")}</div>}
                </div>
              )}
            </div>
            {configSaveNotice && (
              <button type="button" onClick={() => setConfigSaveNotice(null)} aria-label={t("settings.dismissStatus")} className="shrink-0 text-[#999999] hover:text-white">
                <X size={14} />
              </button>
            )}
          </div>
        </div>
      )}
      {playlistItems && (
        <PlaylistPickerModal
          items={playlistItems}
          title={t("playlists.addTrack")}
          onClose={() => setPlaylistItems(null)}
        />
      )}

      <QueueDrawer
        isOpen={showQueue}
        onClose={() => setShowQueue(false)}
        status={playbackStatus}
        isPlaybackAvailable={isPlaybackAvailable}
      />

      {showCollectionWizard && <WelcomeWizard initialConfig={config} devices={devices}
        onCancel={() => setShowCollectionWizard(false)}
        onSaveAccepted={operation => { configSaveMonitorRef.current?.accepted(operation); setShowCollectionWizard(false); }}
        onFinish={newConfig => { setConfig(newConfig); setLocalCatalogRevision(revision => revision + 1); setShowCollectionWizard(false); }} />}

      {showSettings && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          isLibraryUpdating={playbackStatus.is_updating}
          onLibraryRescanned={() => setLocalCatalogRevision((revision) => revision + 1)}
          onSaveStarted={(audioRestartExpected) => {
            if (!beginConfigSave("other")) return false;
            if (audioRestartExpected) invalidateStatusRequests();
            return true;
          }}
          onSaveFinished={finishConfigSave}
          onSaveAccepted={(operation) => configSaveMonitorRef.current?.accepted(operation)}
          onStartLocalFoldersSave={startLocalFoldersSave}
          onSaved={(audioRestartExpected) => {
            if (audioRestartExpected) invalidateStatusRequests();
            configService.getConfig().then(setConfig).catch(error => logError("settings.reload", error));
            refreshPlexFavorites();
          }}
        />
      )}

      {queueClearConfirmation && !configSavePhase && (
        <QueueClearConfirmationDialog
          onCancel={() => cancelQueueClear(queueClearConfirmation.operationId)}
          onConfirm={() => confirmQueueClear(queueClearConfirmation.operationId)}
        />
      )}

      {showAbout && <AboutModal onClose={() => setShowAbout(false)} />}
    </div>
  );
}

export default App;
