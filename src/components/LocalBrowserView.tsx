import React, { useEffect, useState, useRef } from "react";
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
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { LocalItem, LocalAlbum } from "../types/local";
import { FavoriteAlbum } from "../types/favorite";
import { audioService } from "../services/audio";
import { favoritesService } from "../services/favorites";

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

const LocalAlbumCard: React.FC<{
  album: LocalAlbum;
  isFavorite: boolean;
  onToggleFavorite: (e: React.MouseEvent, album: LocalAlbum, cover: string | null) => void;
  onClick: () => void;
  onPlayQuick: (e: React.MouseEvent) => void;
}> = ({ album, isFavorite, onToggleFavorite, onClick, onPlayQuick }) => {
  const [cover, setCover] = useState<string | null>(() => coverMemoryCache.get(album.folder_path) || null);
  const cardRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (coverMemoryCache.has(album.folder_path)) {
      setCover(coverMemoryCache.get(album.folder_path)!);
      return;
    }

    let isMounted = true;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting) {
          audioService.getLocalCover(album.folder_path).then((c) => {
            if (isMounted && c) {
              coverMemoryCache.set(album.folder_path, c);
              setCover(c);
            }
          });
          observer.disconnect();
        }
      },
      { rootMargin: "150px" }
    );

    if (cardRef.current) {
      observer.observe(cardRef.current);
    }

    return () => {
      isMounted = false;
      observer.disconnect();
    };
  }, [album.folder_path]);

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
            className="w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer"
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
}

export const LocalBrowserView: React.FC<LocalBrowserViewProps> = ({
  initialArtist,
  onClearInitialArtist,
}) => {
  const { t } = useTranslation();
  const [viewMode, setViewMode] = useState<"albums" | "folders">("albums");
  const [albums, setAlbums] = useState<LocalAlbum[]>([]);
  const [loadingAlbums, setLoadingAlbums] = useState(false);
  const [albumSearch, setAlbumSearch] = useState("");
  const [selectedAlbum, setSelectedAlbum] = useState<LocalAlbum | null>(null);
  const [selectedArtist, setSelectedArtist] = useState<string | null>(initialArtist || null);
  const [albumTracks, setAlbumTracks] = useState<LocalItem[]>([]);
  const [albumCover, setAlbumCover] = useState<string | null>(null);
  const [favoriteIds, setFavoriteIds] = useState<Set<string>>(new Set());

  const [currentPath, setCurrentPath] = useState<string>("");
  const [items, setItems] = useState<LocalItem[]>([]);
  const [loadingFolders, setLoadingFolders] = useState<boolean>(false);
  const [folderCover, setFolderCover] = useState<string | null>(null);

  useEffect(() => {
    let isMounted = true;
    setLoadingAlbums(true);

    audioService
      .getLocalAlbums()
      .then((data) => {
        if (isMounted) setAlbums(data);
      })
      .catch(console.error)
      .finally(() => {
        if (isMounted) setLoadingAlbums(false);
      });

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
    if (initialArtist) {
      setSelectedArtist(initialArtist);
      setSelectedAlbum(null);
      setViewMode("albums");
    }
  }, [initialArtist]);

  useEffect(() => {
    if (viewMode === "folders") {
      setLoadingFolders(true);
      Promise.all([
        audioService.listLocalDirectory(currentPath),
        audioService.getLocalCover(currentPath),
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
    setSelectedAlbum(album);
    try {
      const cov = coverMemoryCache.get(album.folder_path) || (await audioService.getLocalCover(album.folder_path));
      if (cov) coverMemoryCache.set(album.folder_path, cov);
      setAlbumCover(cov);

      const trackList = await audioService.listLocalDirectory(album.folder_path);
      setAlbumTracks(trackList.filter((i) => i.item_type === "file"));
    } catch (err) {
      console.error("Falha ao carregar faixas do álbum:", err);
    }
  };

  const handlePlayEntireAlbum = async (album: LocalAlbum, trackItems?: LocalItem[], startIdx = 0) => {
    try {
      const files = trackItems || (await audioService.listLocalDirectory(album.folder_path)).filter((i) => i.item_type === "file");
      const cov = albumCover || coverMemoryCache.get(album.folder_path) || (await audioService.getLocalCover(album.folder_path));
      const meta = files.map((f) => ({
        title: f.title || f.name,
        artist: f.artist || album.artist,
        album: f.album || album.title,
        thumb: cov,
        uri: f.path,
      }));
      if (meta.length > 0) {
        audioService.playTracks(meta, startIdx);
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

                <div className="pt-2 flex items-center space-x-3">
                  <button
                    onClick={() => handlePlayEntireAlbum(selectedAlbum, albumTracks, 0)}
                    className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer"
                  >
                    <Play size={16} fill="black" />
                    <span>{t("localBrowser.playAlbum")}</span>
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

              {albumTracks.map((track, idx) => (
                <div
                  key={track.path}
                  onClick={() => handlePlayEntireAlbum(selectedAlbum, albumTracks, idx)}
                  className="grid grid-cols-12 px-4 py-3 text-xs items-center hover:bg-[#1E1E1E] transition-colors cursor-pointer group"
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
                  <span className="col-span-3 text-right font-mono text-[#888888]">
                    {formatDuration(track.duration)}
                  </span>
                </div>
              ))}
            </div>
          </div>
        ) : selectedArtist ? (
          <div className="space-y-6 animate-in fade-in duration-100">
            {artistAlbums.length === 0 ? (
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
                  />
                ))}
              </div>
            )}
          </div>
        ) : viewMode === "albums" ? (
          loadingAlbums ? (
            <div className="h-60 flex items-center justify-center text-xs text-[#666666]">
              {t("localBrowser.organizing")}
            </div>
          ) : filteredAlbums.length === 0 ? (
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
                        } else {
                          const meta = [
                            {
                              title: item.title || item.name,
                              artist: item.artist || "",
                              album: item.album || "",
                              thumb: folderCover,
                              uri: item.path,
                            },
                          ];
                          audioService.playTracks(meta, 0);
                        }
                      }}
                      className="flex items-center justify-between p-3 hover:bg-[#1E1E1E] transition-colors cursor-pointer group"
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
                        <span className="text-xs font-mono text-[#666666]">
                          {formatDuration(item.duration)}
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </div>
    </main>
  );
};

export default LocalBrowserView;
