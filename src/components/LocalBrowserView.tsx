import React, { useEffect, useMemo, useState, useRef, useSyncExternalStore } from "react";
import {
  Folder,
  Music,
  Play,
  ArrowLeft,
  Disc3,
  Search,
  Grid,
  ListTree,
  Users,
  CalendarDays,
  Clock,
  Sparkles,
  Heart,
  ListPlus,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { LocalItem, LocalAlbum } from "../types/local";
import { FavoriteAlbum } from "../types/favorite";
import { audioService } from "../services/audio";
import { favoritesService } from "../services/favorites";
import { PlaylistPickerModal } from "./PlaylistPickerModal";
import type { NewPlaylistItem } from "../types/playlist";
import { flattenAlbumDiscs, type AlbumDiscTracks } from "../utils/localAlbumDiscs";
import { LocalAlbumCatalog, emptyLocalAlbumCatalogState } from "../utils/localAlbumCatalog";
import { albumLocationSources } from "../utils/localAlbumLocations";
import { localArtworkEnrichment, localArtworkResolver } from "../utils/localArtworkSession";
import {
  filterLocalAlbumsByScope,
  localLibrarySourceOptions,
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
import { VirtualAlbumGrid, type VirtualAlbumGridHandle } from "./VirtualAlbumGrid";
import { VirtualList, type VirtualListHandle } from "./VirtualList";

const localAlbumCatalog = new LocalAlbumCatalog(audioService.getLocalAlbums);

async function getLocalAlbumCover(album: LocalAlbum, onlineEnabled: boolean | (() => boolean), libraryUpdating: boolean | (() => boolean)): Promise<string | null> {
  return localArtworkResolver.resolve(album, onlineEnabled, libraryUpdating);
}

async function loadAlbumDiscTracks(album: LocalAlbum): Promise<AlbumDiscTracks[]> {
  return Promise.all(album.discs.map(async (disc) => ({
    disc,
    tracks: (await audioService.listLocalDirectory(disc.folder_path)).filter((item) => item.item_type === "file"),
  })));
}

const LocalAlbumCard: React.FC<{
  album: LocalAlbum;
  isFavorite: boolean;
  onToggleFavorite: (e: React.MouseEvent, album: LocalAlbum, cover: string | null) => void;
  onClick: () => void;
  onPlayQuick: (e: React.MouseEvent) => void;
  isPlaybackAvailable: boolean;
  onlineArtworkEnabled: boolean;
  isLibraryUpdating: boolean;
}> = ({ album, isFavorite, onToggleFavorite, onClick, onPlayQuick, isPlaybackAvailable, onlineArtworkEnabled, isLibraryUpdating }) => {
  const [cover, setCover] = useState<string | null>(() => localArtworkResolver.get(album.id));
  const cardRef = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const onlineEnabledRef = useRef(onlineArtworkEnabled);
  const libraryUpdatingRef = useRef(isLibraryUpdating);
  onlineEnabledRef.current = onlineArtworkEnabled;
  libraryUpdatingRef.current = isLibraryUpdating;

  useEffect(() => {
    const cached = localArtworkResolver.get(album.id);
    if (cached) setCover(cached);
    return localArtworkResolver.subscribe(album.id, setCover);
  }, [album.id]);

  useEffect(() => {
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
  }, [album.folder_path]);

  useEffect(() => {
    if (!visible || cover) return;
    let active = true;
    getLocalAlbumCover(album, () => onlineEnabledRef.current, () => libraryUpdatingRef.current)
      .then((found) => { if (active && found) setCover(found); })
      .catch((error) => console.error("Falha ao carregar capa do álbum:", error));
    return () => { active = false; };
  }, [album, visible, cover, onlineArtworkEnabled, isLibraryUpdating]);

  return (
    <div ref={cardRef} onClick={onClick} className="group flex flex-col cursor-pointer relative">
      <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md">
        {cover ? (
          <img
            src={cover}
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
          onClick={(e) => onToggleFavorite(e, album, cover)}
          className={`absolute top-2 right-2 p-1.5 rounded-full backdrop-blur-xs transition-transform active:scale-90 cursor-pointer shadow z-10 ${
            isFavorite
              ? "bg-black/60 text-[#E5A00D]"
              : "bg-black/40 text-white/70 hover:text-white opacity-0 group-hover:opacity-100"
          }`}
        >
          <Heart size={14} fill={isFavorite ? "#E5A00D" : "none"} />
        </button>

        <div className="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
          <button
            onClick={onPlayQuick}
            disabled={!isPlaybackAvailable}
            className="w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Play size={20} className="ml-1" fill="black" />
          </button>
        </div>
      </div>

      <span className="text-sm font-semibold text-white truncate" title={album.title}>
        {album.title}
      </span>
      <span className="text-xs text-[#999999] truncate mt-0.5" title={album.artist}>
        {album.artist}
      </span>
      <span className="text-[11px] text-[#666666] mt-0.5">
        {album.year ? `${album.year} • ` : ""}
        {album.track_count} {album.track_count === 1 ? "faixa" : "faixas"}
      </span>
    </div>
  );
};

export interface LocalBrowserViewProps {
  initialArtist?: string | null;
  onClearInitialArtist?: () => void;
  isPlaybackAvailable: boolean;
  isLibraryUpdating: boolean;
  onlineArtworkEnabled: boolean;
}

export const LocalBrowserView: React.FC<LocalBrowserViewProps> = ({
  initialArtist,
  onClearInitialArtist,
  isPlaybackAvailable,
  isLibraryUpdating,
  onlineArtworkEnabled,
}) => {
  const { t } = useTranslation();
  const [viewMode, setViewMode] = useState<"albums" | "artists" | "years" | "folders">("albums");
  const catalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const artworkStatus = useSyncExternalStore(localArtworkEnrichment.subscribe, localArtworkEnrichment.getSnapshot);
  const albums = catalog.albums;
  const emptyCatalogState = emptyLocalAlbumCatalogState(catalog, isLibraryUpdating);
  const [albumSearch, setAlbumSearch] = useState("");
  const [artistSearch, setArtistSearch] = useState("");
  const [selectedSourceId, setSelectedSourceId] = useState<string | null>(null);
  const [albumSortMode, setAlbumSortMode] = useState<LocalAlbumSortMode>("album-asc");
  const [artistSortMode, setArtistSortMode] = useState<LocalArtistSortMode>("artist-asc");
  const [selectedDecade, setSelectedDecade] = useState<number | null>(null);
  const [selectedYear, setSelectedYear] = useState<number | null>(null);
  const [unknownYearSelected, setUnknownYearSelected] = useState(false);
  const [selectedAlbum, setSelectedAlbum] = useState<LocalAlbum | null>(null);
  const [albumLocations, setAlbumLocations] = useState<{
    album: LocalAlbum;
    paths: { label: string | null; absolutePath: string }[];
  } | null>(null);
  const [selectedArtist, setSelectedArtist] = useState<string | null>(initialArtist || null);
  const [albumTracks, setAlbumTracks] = useState<LocalItem[]>([]);
  const [albumSections, setAlbumSections] = useState<ReturnType<typeof flattenAlbumDiscs>["sections"]>([]);
  const [albumCover, setAlbumCover] = useState<string | null>(null);
  const albumRequest = useRef(0);
  const albumGridRef = useRef<VirtualAlbumGridHandle>(null);
  const artistListRef = useRef<VirtualListHandle>(null);
  const onlineEnabledRef = useRef(onlineArtworkEnabled);
  const libraryUpdatingRef = useRef(isLibraryUpdating);
  onlineEnabledRef.current = onlineArtworkEnabled;
  libraryUpdatingRef.current = isLibraryUpdating;
  const initialCatalogMountHandled = useRef(false);
  const [favoriteIds, setFavoriteIds] = useState<Set<string>>(new Set());
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);
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
      .catch(console.error);

    return () => {
      isMounted = false;
    };
  }, []);

  useEffect(() => {
    if (!initialCatalogMountHandled.current) {
      initialCatalogMountHandled.current = true;
      localAlbumCatalog.setUpdating(isLibraryUpdating, true);
    }
  }, []);

  useEffect(() => {
    localAlbumCatalog.setUpdating(isLibraryUpdating);
  }, [isLibraryUpdating]);

  useEffect(() => {
    if (catalog.loaded && !catalog.loading && !catalog.error && !isLibraryUpdating) {
      localArtworkEnrichment.setCatalog(catalog.albums);
    }
  }, [catalog.albums, catalog.loaded, catalog.loading, catalog.error, isLibraryUpdating]);

  useEffect(() => {
    if (!selectedAlbum) return;
    const cached = localArtworkResolver.get(selectedAlbum.id);
    if (cached) setAlbumCover(cached);
    return localArtworkResolver.subscribe(selectedAlbum.id, setAlbumCover);
  }, [selectedAlbum]);

  useEffect(() => {
    if (initialArtist) {
      setSelectedArtist(initialArtist);
      setSelectedAlbum(null);
      setViewMode("artists");
    }
  }, [initialArtist]);

  useEffect(() => {
    if (!selectedAlbum) return;
    let active = true;
    const album = selectedAlbum;
    const sources = albumLocationSources(album);
    void Promise.allSettled(
      sources.map((source) => audioService.resolveLocalLibraryPath(source.path)),
    ).then((results) => {
      if (!active) return;
      const paths = results.flatMap((result, index) => {
        if (result.status === "rejected") {
          console.error("Falha ao resolver localização do álbum local:", result.reason);
          return [];
        }
        return [{ label: sources[index].label, absolutePath: result.value }];
      });
      setAlbumLocations({ album, paths });
    });
    return () => {
      active = false;
    };
  }, [selectedAlbum]);

  useEffect(() => {
    if (viewMode === "folders") {
      setLoadingFolders(true);
      Promise.all([
        audioService.listLocalDirectory(currentPath),
        audioService.getLocalCover(currentPath).catch((error) => {
          console.error("Falha ao carregar capa da pasta local:", error);
          return null;
        }),
      ])
        .then(([data, cov]) => {
          setItems(data);
          setFolderCover(cov);
        })
        .catch(console.error)
        .finally(() => setLoadingFolders(false));
    }
  }, [currentPath, viewMode]);

  const handleToggleFavoriteLocal = async (e: React.MouseEvent, album: LocalAlbum, cov: string | null) => {
    e.stopPropagation();
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
      console.error("Erro ao favoritar:", err);
    }
  };

  const handleSelectAlbum = async (album: LocalAlbum) => {
    const request = ++albumRequest.current;
    setSelectedAlbum(album);
    setAlbumLocations(null);
    setAlbumTracks([]);
    setAlbumSections([]);
    setAlbumCover(null);
    void getLocalAlbumCover(album, () => onlineEnabledRef.current, () => libraryUpdatingRef.current)
      .then((cov) => { if (request === albumRequest.current) setAlbumCover(cov); })
      .catch((error) => console.error("Falha ao carregar capa do álbum:", error));
    try {
      const groups = await loadAlbumDiscTracks(album);
      if (request !== albumRequest.current) return;
      const flattened = flattenAlbumDiscs(groups);
      setAlbumTracks(flattened.tracks);
      setAlbumSections(flattened.sections);
    } catch (err) {
      console.error("Falha ao carregar faixas do álbum:", err);
    }
  };

  const handlePlayEntireAlbum = async (album: LocalAlbum, trackItems?: LocalItem[], startIdx = 0) => {
    if (!isPlaybackAvailable) return;
    try {
      const files = trackItems ?? flattenAlbumDiscs(await loadAlbumDiscTracks(album)).tracks;
      const cov = (selectedAlbum?.id === album.id ? albumCover : null) || await localArtworkResolver.resolveLocalOnly(album);
      const meta = files.map((f) => ({
        title: f.title || f.name,
        artist: f.artist || album.artist,
        album: f.album || album.title,
        thumb: cov,
        uri: f.path,
	duration: f.duration,
      }));
      if (meta.length > 0) {
        await audioService.playTracks(meta, startIdx);
      }
    } catch (err) {
      console.error("Erro ao tocar álbum:", err);
    }
  };

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

  const sourceOptions = useMemo(() => localLibrarySourceOptions(albums), [albums]);
  const activeSourceId = validLocalLibrarySourceSelection(selectedSourceId, sourceOptions);
  const scopedAlbums = useMemo(
    () => filterLocalAlbumsByScope(albums, activeSourceId, ""),
    [albums, activeSourceId],
  );
  const filteredAlbums = useMemo(
    () => filterLocalAlbumsByScope(scopedAlbums, null, albumSearch),
    [scopedAlbums, albumSearch],
  );
  const sortedAlbums = useMemo(
    () => sortLocalAlbums(filteredAlbums, albumSortMode),
    [filteredAlbums, albumSortMode],
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
    selectedAlbum
    || selectedArtist
    || selectedDecade !== null
    || selectedYear !== null
    || unknownYearSelected,
  );
  const headerTitle = selectedAlbum?.title
    ?? selectedArtist
    ?? (unknownYearSelected ? t("localBrowser.unknownYear") : null)
    ?? (selectedYear !== null ? String(selectedYear) : null)
    ?? (selectedDecade !== null ? `${selectedDecade}s` : null)
    ?? (viewMode === "albums"
      ? t("localBrowser.albumsTitle")
      : viewMode === "artists"
        ? t("localBrowser.artistsTitle")
        : viewMode === "years"
          ? t("localBrowser.yearsTitle")
          : t("localBrowser.foldersTitle"));
  const headerCount = selectedAlbum
    ? selectedAlbum.artist
    : selectedArtist
      ? t("localBrowser.albumsCount", { count: artistAlbums.length })
      : selectedYear !== null || unknownYearSelected
        ? t("localBrowser.albumsCount", { count: yearAlbums.length })
        : selectedDecadeSummary
          ? t("localBrowser.albumsCount", { count: selectedDecadeSummary.albumCount })
          : viewMode === "albums"
            ? t("localBrowser.albumsCount", { count: filteredAlbums.length })
            : viewMode === "artists"
              ? t("localBrowser.artistsCount", { count: visibleArtists.length })
              : viewMode === "years"
                ? t("localBrowser.albumsCount", { count: scopedAlbums.length })
                : currentPath || t("localBrowser.root");

  const handleContextBack = () => {
    if (selectedAlbum) {
      setSelectedAlbum(null);
    } else if (selectedArtist) {
      setSelectedArtist(null);
      onClearInitialArtist?.();
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
    setSelectedAlbum(null);
    setSelectedArtist(null);
    setSelectedDecade(null);
    setSelectedYear(null);
    setUnknownYearSelected(false);
    onClearInitialArtist?.();
  };

  return (
    <main className="flex-1 flex flex-col overflow-hidden bg-[#121212] select-none">
      <div className="flex flex-wrap items-center justify-between gap-4 px-8 pt-8 pb-4">
        <div className="flex items-center space-x-3">
          {showContextBack && (
            <button
              onClick={handleContextBack}
              className="p-1.5 rounded-lg bg-[#1E1E1E] border border-[#333333] hover:bg-[#2A2A2A] text-white transition-colors cursor-pointer mr-1"
              title={t("localBrowser.back")}
            >
              <ArrowLeft size={16} />
            </button>
          )}

          <div>
            <h2 className="text-2xl font-bold text-white tracking-tight">{headerTitle}</h2>
            <p className="text-xs text-[#888888] mt-0.5">{headerCount}</p>
            {viewMode === "albums" && artworkStatus.onlineActivity && (
              <div className="text-[11px] text-[#777777] mt-1 flex flex-wrap items-center gap-x-2">
                <span className="text-[#E5A00D] flex items-center gap-1 max-w-72 truncate" title={artworkStatus.onlineActivity.title}>
                  {artworkStatus.onlineActivity.kind === "searching" && <Disc3 size={11} className="animate-spin" />}
                  {t(artworkStatus.onlineActivity.kind === "searching" ? "artwork.searchingOnline" : "artwork.foundOnline", { title: artworkStatus.onlineActivity.title })}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>

      {!selectedAlbum && !selectedArtist && (
        <div className="flex flex-wrap gap-1 border-b border-[#222222] px-8 pb-3">
          {([
            ["albums", Grid, t("localBrowser.tabAlbums")],
            ["artists", Users, t("localBrowser.tabArtists")],
            ["years", CalendarDays, t("localBrowser.tabYears")],
            ["folders", ListTree, t("localBrowser.tabFolders")],
          ] as const).map(([mode, Icon, label]) => (
            <button
              key={mode}
              type="button"
              onClick={() => selectViewMode(mode)}
              className={`flex items-center space-x-1.5 rounded-md px-3 py-1.5 text-xs font-semibold transition-all cursor-pointer ${
                viewMode === mode ? "bg-[#E5A00D] text-black shadow" : "text-[#999999] hover:bg-[#1E1E1E] hover:text-white"
              }`}
            >
              <Icon size={14} />
              <span>{label}</span>
            </button>
          ))}
        </div>
      )}

      {!selectedAlbum && viewMode !== "folders" && (
        <div className="flex flex-wrap items-center gap-3 border-b border-[#222222] px-8 py-3">
          <label className="flex items-center gap-2 text-xs text-[#888888]">
            <span>{t("localBrowser.libraryScope")}</span>
            <select
              value={activeSourceId ?? ""}
              onChange={(event) => setSelectedSourceId(event.target.value || null)}
              className="max-w-44 rounded-lg border border-[#2B2B2B] bg-[#1A1A1A] px-3 py-1.5 text-xs text-white outline-none focus:border-[#E5A00D]"
            >
              <option value="">{t("localBrowser.allLibraries")}</option>
              {sourceOptions.map((source) => (
                <option key={source.id} value={source.id}>{source.label}</option>
              ))}
            </select>
          </label>
          {viewMode === "albums" && (
            <>
              <div className="relative flex w-64 max-w-full items-center">
                <Search size={14} className="absolute left-3 text-[#666666]" />
                <input type="text" value={albumSearch} onChange={(event) => setAlbumSearch(event.target.value)} placeholder={t("localBrowser.filterPlaceholder")} className="w-full rounded-lg border border-[#2B2B2B] bg-[#1A1A1A] py-1.5 pl-9 pr-3 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D]" />
              </div>
              <label className="flex items-center gap-2 text-xs text-[#888888]">
                <span>{t("localBrowser.sortLabel")}</span>
                <select value={albumSortMode} onChange={(event) => setAlbumSortMode(event.target.value as LocalAlbumSortMode)} className="max-w-44 rounded-lg border border-[#2B2B2B] bg-[#1A1A1A] px-3 py-1.5 text-xs text-white outline-none focus:border-[#E5A00D]">
                  <option value="album-asc">{t("localBrowser.sortAlbumAsc")}</option>
                  <option value="album-desc">{t("localBrowser.sortAlbumDesc")}</option>
                  <option value="artist-asc">{t("localBrowser.sortArtistAsc")}</option>
                  <option value="artist-desc">{t("localBrowser.sortArtistDesc")}</option>
                  <option value="year-newest">{t("localBrowser.sortYearNewest")}</option>
                  <option value="year-oldest">{t("localBrowser.sortYearOldest")}</option>
                </select>
              </label>
            </>
          )}
          {viewMode === "artists" && !selectedArtist && (
            <>
              <div className="relative flex w-64 max-w-full items-center">
                <Search size={14} className="absolute left-3 text-[#666666]" />
                <input type="text" value={artistSearch} onChange={(event) => setArtistSearch(event.target.value)} placeholder={t("localBrowser.artistSearchPlaceholder")} className="w-full rounded-lg border border-[#2B2B2B] bg-[#1A1A1A] py-1.5 pl-9 pr-3 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D]" />
              </div>
              <label className="flex items-center gap-2 text-xs text-[#888888]">
                <span>{t("localBrowser.sortLabel")}</span>
                <select value={artistSortMode} onChange={(event) => setArtistSortMode(event.target.value as LocalArtistSortMode)} className="max-w-44 rounded-lg border border-[#2B2B2B] bg-[#1A1A1A] px-3 py-1.5 text-xs text-white outline-none focus:border-[#E5A00D]">
                  <option value="artist-asc">{t("localBrowser.sortArtistAsc")}</option>
                  <option value="artist-desc">{t("localBrowser.sortArtistDesc")}</option>
                </select>
              </label>
            </>
          )}
        </div>
      )}

      {!selectedAlbum && !selectedArtist && viewMode === "albums" && isTextLocalAlbumSort(albumSortMode) && (
        <nav
          aria-label={t("localBrowser.albumIndex")}
          className="flex flex-wrap items-center gap-1 border-b border-[#222222] px-8 py-2"
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
                className="h-6 min-w-6 rounded px-1 text-[11px] font-semibold text-[#999999] transition-colors hover:bg-[#282828] hover:text-[#E5A00D] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:cursor-default disabled:text-[#444444] disabled:hover:bg-transparent"
              >
                {bucket}
              </button>
            );
          })}
        </nav>
      )}
      {!selectedAlbum && !selectedArtist && viewMode === "artists" && (
        <nav aria-label={t("localBrowser.artistIndex")} className="flex flex-wrap items-center gap-1 border-b border-[#222222] px-8 py-2">
          {LOCAL_ALBUM_INDEX_BUCKETS.map((bucket) => {
            const index = artistBucketIndices.get(bucket);
            const label = t("localBrowser.jumpToArtistBucket", { bucket });
            return (
              <button key={bucket} type="button" disabled={index === undefined} title={label} aria-label={label} onClick={() => index !== undefined && artistListRef.current?.scrollToIndex(index)} className="h-6 min-w-6 rounded px-1 text-[11px] font-semibold text-[#999999] transition-colors hover:bg-[#282828] hover:text-[#E5A00D] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] disabled:cursor-default disabled:text-[#444444] disabled:hover:bg-transparent">
                {bucket}
              </button>
            );
          })}
        </nav>
      )}

      {isLibraryUpdating && (
        <div className="px-8 py-2 text-xs text-[#E5A00D] flex items-center gap-2 border-b border-[#222222]">
          <Disc3 size={14} className="animate-spin" />
          <span>{t("sidebar.indexingTooltip")}</span>
        </div>
      )}
      {catalog.error && (
        <div role="alert" className="px-8 py-2 text-xs text-red-400 border-b border-[#222222]">
          {catalog.error}
        </div>
      )}
      <div ref={setScrollContainer} className="flex-1 overflow-y-auto p-8">
        {selectedAlbum ? (
          <div className="space-y-8 animate-in fade-in duration-100">
            <div className="flex items-end space-x-6">
              <div className="w-52 h-52 rounded-xl bg-[#202020] border border-[#2B2B2B] overflow-hidden shrink-0 shadow-2xl flex items-center justify-center">
                {albumCover ? (
                  <img src={albumCover} alt={selectedAlbum.title} className="w-full h-full object-cover" />
                ) : (
                  <Disc3 size={64} className="text-[#444444]" />
                )}
              </div>

              <div className="space-y-3">
                <span className="text-xs font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1">
                  <Sparkles size={13} />
                  <span>{t("localBrowser.albumsTitle")}</span>
                </span>
                <h1 className="text-3xl font-black text-white">{selectedAlbum.title}</h1>
                <button
                  type="button"
                  onClick={() => {
                    setSelectedArtist(selectedAlbum.artist);
                    setSelectedAlbum(null);
                    setViewMode("artists");
                    setSelectedDecade(null);
                    setSelectedYear(null);
                    setUnknownYearSelected(false);
                  }}
                  className="text-base text-[#CCCCCC] hover:text-[#E5A00D] font-medium transition-colors cursor-pointer text-left block"
                  title={t("localBrowser.viewDiscography", "Ver discografia")}
                >
                  {selectedAlbum.artist}
                </button>
                <p className="text-xs text-[#777777]">
                  {selectedAlbum.year ? `${selectedAlbum.year} • ` : ""}
                  {albumTracks.length} {t("favorites.tracks")}
                </p>
                {albumLocations?.album === selectedAlbum && albumLocations.paths.length > 0 && (
                  <div className="space-y-1 text-[11px] text-[#777777] min-w-0">
                    <p className="font-semibold">{t("localBrowser.location")}</p>
                    {albumLocations.paths.map((location, index) => (
                      <div key={`${location.absolutePath}-${index}`} className="min-w-0">
                        {location.label && <span className="block text-[#999999]">{location.label}</span>}
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

                <div className="pt-2 flex items-center space-x-3">
                  <button
                    onClick={() => handlePlayEntireAlbum(selectedAlbum, albumTracks.length ? albumTracks : undefined, 0)}
                    disabled={!isPlaybackAvailable}
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
                    className="flex items-center space-x-2 rounded-xl border border-[#2B2B2B] bg-[#1E1E1E] px-4 py-2.5 text-xs font-bold text-white hover:bg-[#282828] disabled:opacity-50"
                    title={t("playlists.addAlbum")}
                  >
                    <ListPlus size={16} />
                    <span>{t("playlists.addAlbum")}</span>
                  </button>

                  <button
                    onClick={(e) => handleToggleFavoriteLocal(e, selectedAlbum, albumCover)}
                    className="p-2.5 rounded-xl bg-[#1E1E1E] border border-[#2B2B2B] hover:bg-[#282828] text-white transition-colors cursor-pointer"
                    title={favoriteIds.has(selectedAlbum.folder_path) ? t("favorites.removeFavorite") : t("favorites.title")}
                  >
                    <Heart
                      size={16}
                      className={favoriteIds.has(selectedAlbum.folder_path) ? "text-[#E5A00D]" : "text-[#888888]"}
                      fill={favoriteIds.has(selectedAlbum.folder_path) ? "#E5A00D" : "none"}
                    />
                  </button>
                </div>
              </div>
            </div>

            <div className="bg-[#141414] border border-[#222222] rounded-xl overflow-hidden divide-y divide-[#1D1D1D]">
              <div className="grid grid-cols-12 px-4 py-2.5 text-[11px] font-bold text-[#666666] uppercase tracking-wider bg-[#181818]">
                <span className="col-span-1 text-center">#</span>
                <span className="col-span-8">{t("plex.tracks")}</span>
                <span className="col-span-3 text-right flex items-center justify-end space-x-1">
                  <Clock size={12} />
                  <span>{t("player.queue")}</span>
                </span>
              </div>

              {albumSections.map((section) => (
                <React.Fragment key={section.disc.folder_path}>
                  {selectedAlbum.discs.length > 1 && (
                    <div className="px-4 py-2.5 text-xs font-bold text-[#E5A00D] bg-[#181818]">
                      {section.disc.label}
                    </div>
                  )}
                  {section.tracks.map((track, idx) => (
                    <div
                      key={track.path}
                      onClick={() => handlePlayEntireAlbum(selectedAlbum, albumTracks, section.startIndex + idx)}
                  aria-disabled={!isPlaybackAvailable}
                  className={`grid grid-cols-12 px-4 py-3 text-xs items-center transition-colors group ${
                    isPlaybackAvailable
                      ? "hover:bg-[#1E1E1E] cursor-pointer"
                      : "opacity-60 cursor-not-allowed"
                  }`}
                >
                  <span className="col-span-1 text-center font-mono text-[#666666] group-hover:text-[#E5A00D]">
                    {idx + 1}
                  </span>
                  <div className="col-span-8 flex flex-col pr-2">
                    <span className="font-semibold text-white group-hover:text-[#E5A00D] transition-colors truncate">
                      {track.title || track.name}
                    </span>
                    <span className="text-[11px] text-[#777777] truncate">
                      {track.artist || selectedAlbum.artist}
                    </span>
                  </div>
                  <div className="col-span-3 flex items-center justify-end gap-3">
                    <span className="font-mono text-[#888888]">
                      {formatDuration(track.duration)}
                    </span>
                    <button
                      type="button"
                      onClick={(event) => {
                        event.stopPropagation();
                        setPlaylistItems([toPlaylistItem(track, selectedAlbum)]);
                      }}
                      className="rounded-md p-1.5 text-[#777777] hover:bg-[#2A2A2A] hover:text-[#E5A00D]"
                      title={t("playlists.addTrack")}
                    >
                      <ListPlus size={15} />
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
              <div className="h-60 flex items-center justify-center text-xs text-[#666666]">
                {t(isLibraryUpdating ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
              </div>
            ) : emptyCatalogState === "error" ? null : artistAlbums.length === 0 ? (
              <div className="h-60 flex flex-col items-center justify-center text-[#666666] space-y-2">
                <Disc3 size={40} className="opacity-40" />
                <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
              </div>
            ) : (
              <VirtualAlbumGrid
                items={artistAlbums}
                getItemKey={(album) => album.id}
                scrollContainer={scrollContainer}
                resetKey={JSON.stringify([activeSourceId, selectedArtist])}
                renderItem={(album) => (
                  <LocalAlbumCard
                    album={album}
                    isFavorite={favoriteIds.has(album.folder_path)}
                    onToggleFavorite={handleToggleFavoriteLocal}
                    onClick={() => handleSelectAlbum(album)}
                    onPlayQuick={(e) => {
                      e.stopPropagation();
                      handlePlayEntireAlbum(album);
                    }}
                    isPlaybackAvailable={isPlaybackAvailable}
                    onlineArtworkEnabled={onlineArtworkEnabled}
                    isLibraryUpdating={isLibraryUpdating}
                  />
                )}
              />
            )}
          </div>
        ) : viewMode === "albums" ? (
          emptyCatalogState === "indexing" ? (
            <div className="h-60 flex items-center justify-center text-xs text-[#666666]">
              {t(isLibraryUpdating ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </div>
          ) : emptyCatalogState === "error" ? null
          : filteredAlbums.length === 0 ? (
            <div className="h-60 flex flex-col items-center justify-center text-[#666666] space-y-2">
              <Disc3 size={40} className="opacity-40" />
              <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
            </div>
          ) : (
            <VirtualAlbumGrid
              ref={albumGridRef}
              items={sortedAlbums}
              getItemKey={(album) => album.id}
              scrollContainer={scrollContainer}
              resetKey={localAlbumNavigationResetKey(activeSourceId, albumSearch, albumSortMode)}
              renderItem={(album) => (
                <LocalAlbumCard
                  album={album}
                  isFavorite={favoriteIds.has(album.folder_path)}
                  onToggleFavorite={handleToggleFavoriteLocal}
                  onClick={() => handleSelectAlbum(album)}
                  onPlayQuick={(e) => {
                    e.stopPropagation();
                    handlePlayEntireAlbum(album);
                  }}
                  isPlaybackAvailable={isPlaybackAvailable}
                  onlineArtworkEnabled={onlineArtworkEnabled}
                  isLibraryUpdating={isLibraryUpdating}
                />
              )}
            />
          )
        ) : viewMode === "artists" ? (
          emptyCatalogState === "indexing" ? (
            <div className="h-60 flex items-center justify-center text-xs text-[#666666]">
              {t(isLibraryUpdating ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </div>
          ) : emptyCatalogState === "error" ? null
          : visibleArtists.length === 0 ? (
            <div className="h-60 flex flex-col items-center justify-center text-[#666666] space-y-2">
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
                  className="flex h-full w-full items-center justify-between border-b border-[#1D1D1D] px-4 text-left transition-colors hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[#E5A00D]"
                >
                  <span className="truncate pr-4 text-sm font-semibold text-white">{artist.name}</span>
                  <span className="shrink-0 text-xs text-[#777777]">{t("localBrowser.albumsCount", { count: artist.albumCount })}</span>
                </button>
              )}
            />
          )
        ) : viewMode === "years" ? (
          selectedYear !== null || unknownYearSelected ? (
            yearAlbums.length === 0 ? (
              <div className="h-60 flex flex-col items-center justify-center text-[#666666] space-y-2">
                <Disc3 size={40} className="opacity-40" />
                <span className="text-xs">{t("localBrowser.emptyAlbums")}</span>
              </div>
            ) : (
              <VirtualAlbumGrid
                items={yearAlbums}
                getItemKey={(album) => album.id}
                scrollContainer={scrollContainer}
                resetKey={JSON.stringify([activeSourceId, selectedYear, unknownYearSelected])}
                renderItem={(album) => (
                  <LocalAlbumCard
                    album={album}
                    isFavorite={favoriteIds.has(album.folder_path)}
                    onToggleFavorite={handleToggleFavoriteLocal}
                    onClick={() => handleSelectAlbum(album)}
                    onPlayQuick={(event) => {
                      event.stopPropagation();
                      handlePlayEntireAlbum(album);
                    }}
                    isPlaybackAvailable={isPlaybackAvailable}
                    onlineArtworkEnabled={onlineArtworkEnabled}
                    isLibraryUpdating={isLibraryUpdating}
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
                  className="rounded-xl border border-[#2B2B2B] bg-[#181818] p-5 text-left transition-colors hover:border-[#E5A00D] hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
                >
                  <span className="block text-xl font-bold text-white">{year.year}</span>
                  <span className="mt-1 block text-xs text-[#777777]">{t("localBrowser.albumsCount", { count: year.albumCount })}</span>
                </button>
              ))}
            </div>
          ) : emptyCatalogState === "indexing" ? (
            <div className="h-60 flex items-center justify-center text-xs text-[#666666]">
              {t(isLibraryUpdating ? "sidebar.indexingTooltip" : "localBrowser.organizing")}
            </div>
          ) : emptyCatalogState === "error" ? null : scopedAlbums.length === 0 ? (
            <div className="h-60 flex flex-col items-center justify-center text-[#666666] space-y-2">
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
                  className="rounded-xl border border-[#2B2B2B] bg-[#181818] p-5 text-left transition-colors hover:border-[#E5A00D] hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
                >
                  <span className="block text-xl font-bold text-white">{decade.decade}s</span>
                  <span className="mt-1 block text-xs text-[#777777]">{t("localBrowser.albumsCount", { count: decade.albumCount })}</span>
                </button>
              ))}
              {yearNavigation.unknownCount > 0 && (
                <button
                  type="button"
                  onClick={() => setUnknownYearSelected(true)}
                  className="rounded-xl border border-[#2B2B2B] bg-[#181818] p-5 text-left transition-colors hover:border-[#E5A00D] hover:bg-[#1E1E1E] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D]"
                >
                  <span className="block text-lg font-bold text-white">{t("localBrowser.unknownYear")}</span>
                  <span className="mt-1 block text-xs text-[#777777]">{t("localBrowser.albumsCount", { count: yearNavigation.unknownCount })}</span>
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
                className="flex items-center space-x-2 text-xs font-semibold text-[#888888] hover:text-white transition-colors cursor-pointer mb-2"
              >
                <ArrowLeft size={14} />
                <span>{t("localBrowser.upOneLevel")}</span>
              </button>
            )}

            {loadingFolders ? (
              <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                {t("localBrowser.loadingFolder")}
              </div>
            ) : items.length === 0 ? (
              <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                {t("localBrowser.emptyFolder")}
              </div>
            ) : (
              <div className="divide-y divide-[#1D1D1D] bg-[#141414] rounded-xl border border-[#222222]">
                {items.map((item) => {
                  const isDir = item.item_type === "directory";
                  return (
                    <div
                      key={item.path}
                      onClick={() => {
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
                          audioService.playTracks(meta, 0);
                        }
                      }}
                      aria-disabled={!isDir && !isPlaybackAvailable}
                      className={`flex items-center justify-between p-3 transition-colors group ${
                        isDir || isPlaybackAvailable
                          ? "hover:bg-[#1E1E1E] cursor-pointer"
                          : "opacity-60 cursor-not-allowed"
                      }`}
                    >
                      <div className="flex items-center space-x-3 truncate mr-4">
                        {isDir ? (
                          <Folder size={18} className="text-[#E5A00D] shrink-0" />
                        ) : (
                          <Music size={18} className="text-[#888888] group-hover:text-white shrink-0" />
                        )}
                        <span className="text-xs font-medium text-white truncate">
                          {item.title || item.name}
                        </span>
                      </div>

                      {!isDir && (
                        <div className="flex items-center gap-3">
                          <span className="text-xs font-mono text-[#666666]">
                            {formatDuration(item.duration)}
                          </span>
                          <button
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              setPlaylistItems([toPlaylistItem(item)]);
                            }}
                            className="rounded-md p-1.5 text-[#777777] hover:bg-[#2A2A2A] hover:text-[#E5A00D]"
                            title={t("playlists.addTrack")}
                          >
                            <ListPlus size={15} />
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
    </main>
  );
};

export default LocalBrowserView;
