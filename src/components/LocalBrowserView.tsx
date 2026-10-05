import React, { useEffect, useState, useRef, useSyncExternalStore } from "react";
import {
  Folder,
  Music,
  Play,
  ArrowLeft,
  Disc3,
  Search,
  Grid,
  ListTree,
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
import { lookupAlbumArtwork } from "../utils/localArtwork";

const localAlbumCatalog = new LocalAlbumCatalog(audioService.getLocalAlbums);

class LruMemoryCache {
  private maxSize: number;
  private map: Map<string, string>;

  constructor(maxSize = 150) {
    this.maxSize = maxSize;
    this.map = new Map();
  }

  get(key: string): string | undefined {
    const val = this.map.get(key);
    if (val !== undefined) {
      this.map.delete(key);
      this.map.set(key, val);
    }
    return val;
  }

  set(key: string, val: string): void {
    if (this.map.has(key)) {
      this.map.delete(key);
    } else if (this.map.size >= this.maxSize) {
      const oldestKey = this.map.keys().next().value;
      if (oldestKey !== undefined) {
        this.map.delete(oldestKey);
      }
    }
    this.map.set(key, val);
  }

  has(key: string): boolean {
    return this.map.has(key);
  }
}

const coverMemoryCache = new LruMemoryCache(150);

async function getLocalAlbumCover(album: LocalAlbum, onlineEnabled: boolean | (() => boolean), libraryUpdating: boolean | (() => boolean)): Promise<string | null> {
  const cached = coverMemoryCache.get(album.id);
  if (cached) return cached;
  const cover = await lookupAlbumArtwork(
    album, audioService.getLocalCover, audioService.getOnlineAlbumCover, onlineEnabled, libraryUpdating,
  );
  if (cover) coverMemoryCache.set(album.id, cover);
  return cover;
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
  const [cover, setCover] = useState<string | null>(() => coverMemoryCache.get(album.id) || null);
  const cardRef = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const onlineEnabledRef = useRef(onlineArtworkEnabled);
  const libraryUpdatingRef = useRef(isLibraryUpdating);
  onlineEnabledRef.current = onlineArtworkEnabled;
  libraryUpdatingRef.current = isLibraryUpdating;

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
  const [viewMode, setViewMode] = useState<"albums" | "folders">("albums");
  const catalog = useSyncExternalStore(localAlbumCatalog.subscribe, localAlbumCatalog.getSnapshot);
  const albums = catalog.albums;
  const emptyCatalogState = emptyLocalAlbumCatalogState(catalog, isLibraryUpdating);
  const [albumSearch, setAlbumSearch] = useState("");
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
  const onlineEnabledRef = useRef(onlineArtworkEnabled);
  const libraryUpdatingRef = useRef(isLibraryUpdating);
  onlineEnabledRef.current = onlineArtworkEnabled;
  libraryUpdatingRef.current = isLibraryUpdating;
  const initialCatalogMountHandled = useRef(false);
  const [favoriteIds, setFavoriteIds] = useState<Set<string>>(new Set());
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);

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
    if (initialArtist) {
      setSelectedArtist(initialArtist);
      setSelectedAlbum(null);
      setViewMode("albums");
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
      const cov = (selectedAlbum?.id === album.id ? albumCover : null) || await getLocalAlbumCover(album, false, () => libraryUpdatingRef.current);
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

  const filteredAlbums = albums.filter(
    (a) =>
      a.title.toLowerCase().includes(albumSearch.toLowerCase()) ||
      a.artist.toLowerCase().includes(albumSearch.toLowerCase())
  );

  const artistAlbums = selectedArtist
    ? albums.filter(
        (a) =>
          a.artist &&
          a.artist.trim().toLowerCase() === selectedArtist.trim().toLowerCase()
      )
    : [];

  return (
    <main className="flex-1 flex flex-col overflow-hidden bg-[#121212] select-none">
      <div className="flex items-center justify-between p-8 pb-4 border-b border-[#222222]">
        <div className="flex items-center space-x-3">
          {(selectedAlbum || selectedArtist) && (
            <button
              onClick={() => {
                if (selectedAlbum) {
                  setSelectedAlbum(null);
                } else if (selectedArtist) {
                  setSelectedArtist(null);
                  if (onClearInitialArtist) onClearInitialArtist();
                }
              }}
              className="p-1.5 rounded-lg bg-[#1E1E1E] border border-[#333333] hover:bg-[#2A2A2A] text-white transition-colors cursor-pointer mr-1"
              title={t("localBrowser.back")}
            >
              <ArrowLeft size={16} />
            </button>
          )}

          <div>
            <h2 className="text-2xl font-bold text-white tracking-tight">
              {selectedAlbum
                ? selectedAlbum.title
                : selectedArtist
                ? selectedArtist
                : viewMode === "albums"
                ? t("localBrowser.albumsTitle")
                : t("localBrowser.foldersTitle")}
            </h2>
            <p className="text-xs text-[#888888] mt-0.5">
              {selectedAlbum
                ? selectedAlbum.artist
                : selectedArtist
                ? t("localBrowser.albumsCount", { count: artistAlbums.length })
                : viewMode === "albums"
                ? t("localBrowser.albumsCount", { count: filteredAlbums.length })
                : currentPath || t("localBrowser.root")}
            </p>
          </div>
        </div>

        {!selectedAlbum && !selectedArtist && (
          <div className="flex items-center space-x-4">
            {viewMode === "albums" && (
              <div className="relative flex items-center w-64">
                <Search size={14} className="absolute left-3 text-[#666666]" />
                <input
                  type="text"
                  value={albumSearch}
                  onChange={(e) => setAlbumSearch(e.target.value)}
                  placeholder={t("localBrowser.filterPlaceholder")}
                  className="w-full bg-[#1A1A1A] border border-[#2B2B2B] rounded-lg pl-9 pr-3 py-1.5 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D] transition-colors"
                />
              </div>
            )}

            <div className="flex bg-[#1E1E1E] p-1 rounded-lg border border-[#333333]">
              <button
                onClick={() => {
                  setViewMode("albums");
                  setSelectedAlbum(null);
                  setSelectedArtist(null);
                }}
                className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                  viewMode === "albums"
                    ? "bg-[#E5A00D] text-black shadow"
                    : "text-[#999999] hover:text-white"
                }`}
              >
                <Grid size={14} />
                <span>{t("localBrowser.tabAlbums")}</span>
              </button>

              <button
                onClick={() => {
                  setViewMode("folders");
                  setSelectedAlbum(null);
                  setSelectedArtist(null);
                }}
                className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                  viewMode === "folders"
                    ? "bg-[#E5A00D] text-black shadow"
                    : "text-[#999999] hover:text-white"
                }`}
              >
                <ListTree size={14} />
                <span>{t("localBrowser.tabFolders")}</span>
              </button>
            </div>
          </div>
        )}
      </div>

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
      <div className="flex-1 overflow-y-auto p-8">
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
              <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                {artistAlbums.map((album) => (
                  <LocalAlbumCard
                    key={album.id || album.folder_path}
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
                ))}
              </div>
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
            <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
              {filteredAlbums.map((album) => (
                <LocalAlbumCard
                  key={album.id}
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
              ))}
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
