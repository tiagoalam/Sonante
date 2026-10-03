import React, { useEffect, useState, useRef, useCallback, memo } from "react";
import {
  Disc3,
  Folder,
  Server,
  Sparkles,
  Play,
  ArrowUpDown,
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
  ExternalLink,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlayerBar } from "./components/PlayerBar";
import { AlbumView } from "./components/AlbumView";
import { ArtistView } from "./components/ArtistView";
import { QueueDrawer } from "./components/QueueDrawer";
import { LocalBrowserView } from "./components/LocalBrowserView";
import { FavoritesView } from "./components/FavoritesView";
import { SettingsModal } from "./components/SettingsModal";
import { AboutModal } from "./components/AboutModal";
import { WelcomeWizard } from "./components/WelcomeWizard";
import { PlexImage } from "./components/PlexImage";
import { plexService } from "./services/plex";
import { audioService } from "./services/audio";
import { configService } from "./services/config";
import { favoritesService } from "./services/favorites";
import { plexCollectionCacheKey } from "./services/plexArtworkCache";
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
          if (!disposed) console.error("Falha ao carregar capas da coleção Plex:", err);
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
          if (album.artist_rating_key) {
            onSelectArtist(album.artist_rating_key, album.artist);
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
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [mediaSource, setMediaSource] = useState<"plex" | "local" | "favorites">("local");
  const [localSelectedArtist, setLocalSelectedArtist] = useState<string | null>(null);
  const [libraries, setLibraries] = useState<PlexLibrary[]>([]);
  const [loadingLibraries, setLoadingLibraries] = useState(true);
  const [selectedLibrary, setSelectedLibrary] = useState<PlexLibrary | null>(null);
  const [activeTab, setActiveTab] = useState<"library" | "collections">("library");
  const [hasCollections, setHasCollections] = useState(false);
  const [sortBy, setSortBy] = useState<string>("added");
  const [showSettings, setShowSettings] = useState(false);
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

  const statusRef = useRef(playbackStatus);
  useEffect(() => {
    statusRef.current = playbackStatus;
  }, [playbackStatus]);
  const playbackAvailableRef = useRef(isPlaybackAvailable);
  useEffect(() => {
    playbackAvailableRef.current = isPlaybackAvailable;
  }, [isPlaybackAvailable]);

  const invalidateStatusRequests = useCallback(() => {
    statusRequestGenerationRef.current += 1;
    setMpdHealth({ state: "starting" });
  }, []);

  // Listagens Plex
  const [albums, setAlbums] = useState<PlexAlbum[]>([]);
  const [collections, setCollections] = useState<PlexCollection[]>([]);

  // Navegação interna Plex
  const [activeCollection, setActiveCollection] = useState<PlexCollection | null>(null);
  const [collectionAlbums, setCollectionAlbums] = useState<PlexAlbum[]>([]);
  const [activeAlbum, setActiveAlbum] = useState<PlexAlbum | null>(null);
  const [activeArtist, setActiveArtist] = useState<SelectedArtist | null>(null);

  // Busca Global
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<PlexSearchResults | null>(null);
  const [isSearching, setIsSearching] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const [loading, setLoading] = useState(false);

  const refreshPlexFavorites = useCallback(() => {
    favoritesService
      .getFavorites()
      .then((favs) => {
        const ids = favs
          .filter((f) => f.source === "plex" && f.id && f.id.trim().length > 0)
          .map((f) => String(f.id));
        setPlexFavIds(new Set(ids));
      })
      .catch(console.error);
  }, []);

  // Carga inicial de configurações e dispositivos
  useEffect(() => {
    Promise.all([configService.getConfig(), configService.getAudioDevices()])
      .then(([cfg, devs]) => {
        setConfig(cfg);
        setDevices(devs);
        if (cfg.plex_token && cfg.plex_token.trim().length > 0) {
          setMediaSource("plex");
        } else {
          setMediaSource("local");
        }
      })
      .catch(console.error);
  }, []);

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
          console.error("Erro ao consultar saúde do motor de áudio:", err);
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
      audioService.setWindowTitle(`Sonante • ${trackLabel}`).catch(console.error);
    } else {
      audioService.setWindowTitle("Sonante").catch(console.error);
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
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
        return;
      }

      if (e.key === "Escape") {
        if (showAbout) {
          setShowAbout(false);
        } else if (showSettings) {
          setShowSettings(false);
        } else if (showQueue) {
          setShowQueue(false);
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
        audioService.togglePlay().catch(console.error);
      } else if (e.code === "ArrowRight") {
        e.preventDefault();
        audioService.next().catch(console.error);
      } else if (e.code === "ArrowLeft") {
        e.preventDefault();
        audioService.previous().catch(console.error);
      } else if (e.code === "ArrowUp") {
        e.preventDefault();
        if (!current.volume.available || !current.volume.writable) return;
        const nextVol = Math.min(current.volume.value + 5, 100);
        audioService.setVolume(nextVol).catch(console.error);
      } else if (e.code === "ArrowDown") {
        e.preventDefault();
        if (!current.volume.available || !current.volume.writable) return;
        const prevVol = Math.max(current.volume.value - 5, 0);
        audioService.setVolume(prevVol).catch(console.error);
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        if (!current.volume.available || !current.volume.writable) return;
        const currentVol = current.volume.value;
        audioService
          .setVolume(!current.volume.muted && currentVol > 0 ? 0 : currentVol || 100)
          .catch(console.error);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [showSettings, showAbout, showQueue, searchQuery]);

  // Carregar ou Limpar Bibliotecas do Plex
  useEffect(() => {
    if (!config?.plex_token || config.plex_token.trim().length === 0) {
      setLoadingLibraries(false);
      setLibraries([]);
      setSelectedLibrary(null);
      setAlbums([]);
      setCollections([]);
      setActiveAlbum(null);
      setActiveCollection(null);
      setActiveArtist(null);
      return;
    }

    let disposed = false;
    setLoadingLibraries(true);
    plexService
      .getLibraries()
      .then((libs) => {
        if (disposed) return;
        setLibraries(libs);
        if (libs.length > 0) {
          setSelectedLibrary(libs[0]);
        }
      })
      .catch((err) => {
        if (!disposed) console.error(err);
      })
      .finally(() => {
        if (!disposed) setLoadingLibraries(false);
      });

    return () => {
      disposed = true;
    };
  }, [config?.plex_token]);

  // Atualizar coleções da biblioteca ativa
  useEffect(() => {
    if (!selectedLibrary) return;
    setActiveAlbum(null);
    setActiveCollection(null);
    setActiveArtist(null);
    setSearchQuery("");
    setSearchResults(null);

    plexService
      .getCollections(selectedLibrary.key)
      .then((cols) => {
        const available = cols.length > 0;
        setHasCollections(available);
        if (!available && activeTab === "collections") {
          setActiveTab("library");
        }
      })
      .catch(() => setHasCollections(false));
  }, [selectedLibrary, activeTab]);

  // Carregar álbuns/coleções do Plex
  useEffect(() => {
    if (!selectedLibrary || searchQuery.trim().length > 0 || mediaSource !== "plex") return;

    setLoading(true);
    if (activeTab === "library") {
      plexService
        .getAlbums(selectedLibrary.key, sortBy)
        .then(setAlbums)
        .catch(console.error)
        .finally(() => setLoading(false));
    } else {
      plexService
        .getCollections(selectedLibrary.key)
        .then(setCollections)
        .catch(console.error)
        .finally(() => setLoading(false));
    }
  }, [selectedLibrary, activeTab, sortBy, searchQuery, mediaSource]);

  // Busca Plex
  useEffect(() => {
    const q = searchQuery.trim();
    if (q.length === 0 || mediaSource !== "plex") {
      setSearchResults(null);
      setIsSearching(false);
      return;
    }

    setIsSearching(true);
    const timer = setTimeout(() => {
      plexService
        .search(q, selectedLibrary?.key)
        .then((res) => setSearchResults(res))
        .catch(console.error)
        .finally(() => setIsSearching(false));
    }, 300);

    return () => clearTimeout(timer);
  }, [searchQuery, selectedLibrary, mediaSource]);

  const handleSelectCollection = async (col: PlexCollection) => {
    setActiveCollection(col);
    setLoading(true);
    try {
      const items = await getCachedCollectionAlbums(col.server_id, col.rating_key, true);
      setCollectionAlbums(items);
    } catch (err) {
      console.error("Falha ao carregar álbuns da coleção:", err);
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
      console.error("Falha na reprodução rápida:", err);
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
      console.error("Erro ao favoritar no Plex:", err);
    }
  }, []);

  const handleSelectArtist = useCallback((artistKey: string, artistName: string) => {
    setActiveArtist({ rating_key: artistKey, name: artistName });
  }, []);

  const resetAllNavigation = () => {
    setLocalSelectedArtist(null);
    setActiveAlbum(null);
    setActiveCollection(null);
    setActiveArtist(null);
    setSearchQuery("");
    setSearchResults(null);
  };

  const isPlexConnected = Boolean(config?.plex_token && config.plex_token.trim().length > 0);

  if (config && config.first_run) {
    return (
      <WelcomeWizard
        initialConfig={config}
        devices={devices}
        onFinish={(newConfig) => {
          setConfig(newConfig);
          if (newConfig.plex_token && newConfig.plex_token.trim().length > 0) {
            setMediaSource("plex");
          } else {
            setMediaSource("local");
          }
        }}
      />
    );
  }

  return (
    <div className="h-screen w-screen flex flex-col bg-[#121212] text-[#E0E0E0] overflow-hidden select-none">
      <div className="flex-1 flex overflow-hidden">
        {/* Barra Lateral */}
        <aside className="w-64 bg-[#181818] border-r border-[#262626] flex flex-col p-4 shrink-0">
          <div className="flex items-center space-x-2.5 px-2 py-3 mb-6">
            <Sparkles className="text-[#E5A00D]" size={22} />
            <h1 className="text-lg font-black tracking-wider text-[#E5A00D]">SONANTE</h1>
          </div>

          <div className="text-[11px] font-bold text-[#666666] tracking-wider uppercase px-2 mb-2">
            {t("sidebar.mediaSources")}
          </div>

          <nav className="space-y-1 mb-6">
            <button
              onClick={() => {
                resetAllNavigation();
                setMediaSource("local");
              }}
              className={`w-full flex items-center justify-between px-3 py-2.5 rounded-md text-sm font-medium transition-colors cursor-pointer ${
                mediaSource === "local"
                  ? "bg-[#242424] text-white"
                  : "text-[#888888] hover:bg-[#202020] hover:text-white"
              }`}
            >
              <div className="flex items-center space-x-3">
                <Folder size={16} className={mediaSource === "local" ? "text-[#E5A00D]" : ""} />
                <span>{t("sidebar.local")}</span>
              </div>

              {!isPlaybackAvailable ? (
                <div
                  className="flex items-center text-[#C9A45D]"
                  title={mpdHealth.state === "unavailable" ? t("player.engineUnavailable") : t("player.engineTransitioning")}
                >
                  <CircleAlert size={13} />
                </div>
              ) : playbackStatus.is_updating ? (
                <div
                  className="flex items-center space-x-1 text-[#E5A00D]"
                  title={t("sidebar.indexingTooltip")}
                >
                  <RefreshCw size={13} className="animate-spin" />
                </div>
              ) : (
                <div
                  className="flex items-center text-[#4BB543]/80"
                  title={t("sidebar.syncedTooltip")}
                >
                  <CheckCircle2 size={13} />
                </div>
              )}
            </button>

            <button
              onClick={() => {
                resetAllNavigation();
                setMediaSource("plex");
              }}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-md text-sm font-medium transition-colors cursor-pointer ${
                mediaSource === "plex"
                  ? "bg-[#242424] text-white"
                  : "text-[#888888] hover:bg-[#202020] hover:text-white"
              }`}
            >
              <Server size={16} className={mediaSource === "plex" ? "text-[#E5A00D]" : ""} />
              <span>{t("sidebar.plex")}</span>
            </button>

            <button
              onClick={() => {
                resetAllNavigation();
                setMediaSource("favorites");
              }}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-md text-sm font-medium transition-colors cursor-pointer ${
                mediaSource === "favorites"
                  ? "bg-[#242424] text-white"
                  : "text-[#888888] hover:bg-[#202020] hover:text-white"
              }`}
            >
              <Heart
                size={16}
                className={mediaSource === "favorites" ? "text-[#E5A00D]" : ""}
                fill={mediaSource === "favorites" ? "#E5A00D" : "none"}
              />
              <span>{t("sidebar.favorites")}</span>
            </button>
          </nav>

          {mediaSource === "plex" && (
            <>
              <div className="text-[11px] font-bold text-[#666666] tracking-wider uppercase px-2 mb-2">
                {t("sidebar.plexAudioLibraries")}
              </div>

              {!isPlexConnected ? (
                <div className="px-3 py-3.5 bg-[#141414] border border-[#242424] rounded-xl text-center space-y-2">
                  <span className="text-[11px] text-[#777777] block">{t("sidebar.noAccountConnected")}</span>
                  <button
                    onClick={() => setShowSettings(true)}
                    className="w-full py-1.5 px-3 bg-[#242424] hover:bg-[#2D2D2D] text-[#E5A00D] rounded-lg text-xs font-bold transition-colors cursor-pointer"
                  >
                    {t("sidebar.connectNow")}
                  </button>
                </div>
              ) : (
                <div className="flex-1 overflow-y-auto space-y-1 pr-1 text-sm">
                  {loadingLibraries ? (
                    <span className="text-xs text-[#666666] px-2 block">{t("sidebar.loadingLibraries")}</span>
                  ) : libraries.length === 0 ? (
                    <span className="text-xs text-[#666666] px-2 block">{t("sidebar.noLibraries")}</span>
                  ) : (
                    libraries.map((lib) => {
                      const isSelected = selectedLibrary?.key === lib.key;
                      return (
                        <button
                          key={lib.key}
                          onClick={() => {
                            setSelectedLibrary(lib);
                            resetAllNavigation();
                          }}
                          className={`w-full text-left px-3 py-2 rounded-md truncate transition-colors cursor-pointer ${
                            isSelected
                              ? "bg-[#332B15] text-[#E5A00D] font-bold"
                              : "text-[#CCCCCC] hover:bg-[#202020] hover:text-white"
                          }`}
                        >
                          {lib.title}
                        </button>
                      );
                    })
                  )}
                </div>
              )}
            </>
          )}

          {mediaSource !== "plex" && <div className="flex-1" />}

          <div className="pt-3 border-t border-[#262626] mt-auto space-y-1">
            <button
              onClick={() => setShowSettings(true)}
              className="w-full flex items-center space-x-3 px-3 py-2 rounded-md text-xs font-semibold text-[#888888] hover:bg-[#202020] hover:text-white transition-colors cursor-pointer"
            >
              <Settings size={15} />
              <span>{t("sidebar.preferences")}</span>
            </button>

            <button
              onClick={() => setShowAbout(true)}
              className="w-full flex items-center space-x-3 px-3 py-2 rounded-md text-xs font-semibold text-[#888888] hover:bg-[#202020] hover:text-[#E5A00D] transition-colors cursor-pointer"
            >
              <Info size={15} />
              <span>{t("sidebar.about")}</span>
            </button>
          </div>
        </aside>

        {/* Painel Central */}
        {mediaSource === "favorites" ? (
          <FavoritesView
            onFavoritesChanged={refreshPlexFavorites}
            isPlaybackAvailable={isPlaybackAvailable}
          />
        ) : mediaSource === "local" ? (
          <LocalBrowserView
            initialArtist={localSelectedArtist}
            onClearInitialArtist={() => setLocalSelectedArtist(null)}
            isPlaybackAvailable={isPlaybackAvailable}
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
              onClick={() => setShowSettings(true)}
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
              setActiveAlbum(null);
              setActiveArtist(art);
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
            <div className="flex items-center justify-between p-8 pb-4 border-b border-[#222222]">
              <div className="flex items-center space-x-3">
                {activeCollection && (
                  <button
                    onClick={() => setActiveCollection(null)}
                    className="p-1.5 rounded-lg bg-[#1E1E1E] border border-[#333333] hover:bg-[#2A2A2A] text-white transition-colors cursor-pointer mr-1"
                    title={t("plex.backToCollections")}
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
                      : selectedLibrary?.title || t("plex.loading")}
                  </h2>
                </div>
              </div>

              <div className="flex items-center space-x-4">
                <div className="relative flex items-center w-72">
                  <Search size={15} className="absolute left-3 text-[#666666]" />
                  <input
                    ref={searchInputRef}
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder={t("plex.searchPlaceholder")}
                    className="w-full bg-[#1A1A1A] border border-[#2B2B2B] rounded-lg pl-9 pr-8 py-1.5 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D] transition-colors"
                  />
                  {searchQuery && (
                    <button
                      onClick={() => setSearchQuery("")}
                      className="absolute right-2.5 text-[#666666] hover:text-white cursor-pointer"
                    >
                      <X size={14} />
                    </button>
                  )}
                </div>

                {!activeCollection && searchQuery.trim().length === 0 && (
                  <>
                    {activeTab === "library" && (
                      <div className="flex items-center space-x-2 bg-[#1E1E1E] px-3 py-1.5 rounded-lg border border-[#333333] text-xs text-[#CCCCCC]">
                        <ArrowUpDown size={14} className="text-[#888888]" />
                        <select
                          value={sortBy}
                          onChange={(e) => setSortBy(e.target.value)}
                          className="bg-transparent border-none outline-none text-white cursor-pointer"
                        >
                          <option value="added">{t("plex.sortAdded")}</option>
                          <option value="title">{t("plex.sortTitle")}</option>
                          <option value="year">{t("plex.sortYear")}</option>
                        </select>
                      </div>
                    )}

                    <div className="flex bg-[#1E1E1E] p-1 rounded-lg border border-[#333333]">
                      <button
                        onClick={() => {
                          setActiveTab("library");
                          setActiveCollection(null);
                        }}
                        className={`px-4 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                          activeTab === "library"
                            ? "bg-[#E5A00D] text-black shadow"
                            : "text-[#999999] hover:text-white"
                        }`}
                      >
                        {t("plex.tabLibrary")}
                      </button>

                      {hasCollections && (
                        <button
                          onClick={() => {
                            setActiveTab("collections");
                            setActiveCollection(null);
                          }}
                          className={`px-4 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                            activeTab === "collections"
                              ? "bg-[#E5A00D] text-black shadow"
                              : "text-[#999999] hover:text-white"
                          }`}
                        >
                          {t("plex.tabCollections")}
                        </button>
                      )}
                    </div>
                  </>
                )}
              </div>
            </div>

            <div className="flex-1 overflow-y-auto p-8">
              {searchQuery.trim().length > 0 ? (
                isSearching ? (
                  <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                    {t("plex.searching")}
                  </div>
                ) : searchResults &&
                  (searchResults.artists.length > 0 ||
                    searchResults.albums.length > 0 ||
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

                    {searchResults.albums.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          {t("plex.albums")}
                        </h3>
                        <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                          {searchResults.albums.map((album) => {
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
                                addFavText={t("favorites.title")}
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
                              <Play size={14} className="text-[#888888] group-hover:text-white shrink-0 mr-2" />
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
                        addFavText={t("favorites.title")}
                        playAlbumText={t("plex.playAlbum")}
                        isPlaybackAvailable={isPlaybackAvailable}
                      />
                    );
                  })}
                </div>
              ) : activeTab === "library" ? (
                <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                  {albums.map((album) => {
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
                        addFavText={t("favorites.title")}
                        playAlbumText={t("plex.playAlbum")}
                        isPlaybackAvailable={isPlaybackAvailable}
                      />
                    );
                  })}
                </div>
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

      <PlayerBar
        status={playbackStatus}
        health={mpdHealth}
        onToggleQueue={() => setShowQueue(!showQueue)}
        isQueueOpen={showQueue}
        onNavigateToArtist={(artistName) => {
          if (playbackStatus.current_media?.kind !== "plex") {
            resetAllNavigation();
            setMediaSource("local");
            setLocalSelectedArtist(artistName);
          } else {
            resetAllNavigation();
            setMediaSource("plex");
            setSearchQuery(artistName);
          }
        }}
        onNavigateToAlbum={() => {
          if (playbackStatus.current_media?.kind !== "plex") {
            resetAllNavigation();
            setMediaSource("local");
          } else if (playbackStatus.album) {
            resetAllNavigation();
            setMediaSource("plex");
            setSearchQuery(playbackStatus.album);
          }
        }}
      />

      <QueueDrawer
        isOpen={showQueue}
        onClose={() => setShowQueue(false)}
        status={playbackStatus}
        isPlaybackAvailable={isPlaybackAvailable}
      />

      {showSettings && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          onSaveStarted={invalidateStatusRequests}
          onSaved={() => {
            invalidateStatusRequests();
            configService.getConfig().then(setConfig).catch(console.error);
            refreshPlexFavorites();
          }}
        />
      )}

      {showAbout && <AboutModal onClose={() => setShowAbout(false)} />}
    </div>
  );
}

export default App;
