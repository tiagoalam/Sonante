import React, { useEffect, useState } from "react";
import {
  Heart,
  Play,
  Disc3,
  Search,
  HardDrive,
  Server,
  ArrowLeft,
  Clock,
  AlertTriangle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { FavoriteAlbum } from "../types/favorite";
import { favoritesService } from "../services/favorites";
import { audioService } from "../services/audio";
import { plexService } from "../services/plex";
import { PlexImage } from "./PlexImage";

interface Props {
  onFavoritesChanged?: () => void;
  isPlaybackAvailable: boolean;
}

export const FavoritesView: React.FC<Props> = ({ onFavoritesChanged, isPlaybackAvailable }) => {
  const { t } = useTranslation();
  const [favorites, setFavorites] = useState<FavoriteAlbum[]>([]);
  const [activeTab, setActiveTab] = useState<"local" | "plex">("local");
  const [search, setSearch] = useState("");
  const [loading, setLoading] = useState(true);

  const [selectedAlbum, setSelectedAlbum] = useState<FavoriteAlbum | null>(null);
  const [tracks, setTracks] = useState<{
    title: string;
    artist: string;
    duration?: number;
    uri?: string;
    media_locator?: import("../types/audio").MediaLocator;
  }[]>([]);
  const [loadingTracks, setLoadingTracks] = useState(false);

  const loadFavorites = async () => {
    try {
      const data = await favoritesService.getFavorites();
      setFavorites(data || []);
    } catch (err) {
      console.error("Falha ao carregar favoritos:", err);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadFavorites();
  }, []);

  const handleToggleFav = async (e: React.MouseEvent, fav: FavoriteAlbum) => {
    e.stopPropagation();
    try {
      await favoritesService.toggleFavorite(fav);
      setFavorites((prev) => prev.filter((item) => !(item.id === fav.id && item.source === fav.source)));
      if (selectedAlbum?.id === fav.id && selectedAlbum?.source === fav.source) {
        setSelectedAlbum(null);
      }
      if (onFavoritesChanged) {
        onFavoritesChanged();
      }
    } catch (err) {
      console.error("Erro ao alterar favorito:", err);
    }
  };

  const handleOpenAlbum = async (fav: FavoriteAlbum) => {
    if (fav.source === "local" && fav.exists === false) {
      alert(t("favorites.missingDiskAlert"));
      return;
    }

    setSelectedAlbum(fav);
    setLoadingTracks(true);
    try {
      if (fav.source === "local") {
        const localFiles = await audioService.listLocalDirectory(fav.path_or_key);
        const fileOnly = localFiles.filter((i) => i.item_type === "file");
        setTracks(
          fileOnly.map((f) => ({
            title: f.title || f.name,
            artist: f.artist || fav.artist,
            duration: f.duration,
            uri: f.path,
          }))
        );
      } else {
        const plexTracks = await plexService.getAlbumTracks(fav.path_or_key);
        setTracks(
          plexTracks.map((t) => ({
            title: t.title,
            artist: fav.artist,
            duration: t.duration ? t.duration / 1000 : undefined,
            media_locator: t.media_locator,
          }))
        );
      }
    } catch (err) {
      console.error("Falha ao carregar faixas do favorito:", err);
    } finally {
      setLoadingTracks(false);
    }
  };

  const handlePlayAll = (startIndex = 0) => {
    if (!isPlaybackAvailable) return;
    if (!selectedAlbum || tracks.length === 0) return;
    const meta = tracks.map((t) => ({
      title: t.title,
      artist: t.artist,
      album: selectedAlbum.title,
      thumb: selectedAlbum.source === "local" ? selectedAlbum.thumb || null : null,
      plex_image: selectedAlbum.source === "plex" ? selectedAlbum.plex_image || null : null,
      uri: t.uri,
      media_locator: t.media_locator,
    }));
    audioService.playTracks(meta, startIndex);
  };

  const handleQuickPlayCard = async (e: React.MouseEvent, fav: FavoriteAlbum) => {
    e.stopPropagation();
    if (!isPlaybackAvailable) return;
    if (fav.source === "local" && fav.exists === false) {
      alert(t("favorites.missingDiskAlert"));
      return;
    }

    try {
      if (fav.source === "local") {
        const items = await audioService.listLocalDirectory(fav.path_or_key);
        const files = items.filter((i) => i.item_type === "file");
        const meta = files.map((f) => ({
          title: f.title || f.name,
          artist: f.artist || fav.artist,
          album: fav.title,
          thumb: fav.thumb || null,
          uri: f.path,
        }));
        if (meta.length > 0) audioService.playTracks(meta, 0);
      } else {
        const plexTracks = await plexService.getAlbumTracks(fav.path_or_key);
        const meta = plexTracks.map((t) => ({
          title: t.title,
          artist: fav.artist,
          album: fav.title,
          plex_image: fav.plex_image || null,
          media_locator: t.media_locator,
        }));
        if (meta.length > 0) audioService.playTracks(meta, 0);
      }
    } catch (err) {
      console.error("Erro na reprodução rápida de favoritos:", err);
    }
  };

  const formatDuration = (secs?: number) => {
    if (!secs || isNaN(secs)) return "--:--";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const currentList = favorites.filter((f) => f.source === activeTab && Boolean(f.id));
  const filtered = currentList.filter(
    (f) =>
      (f.title || "").toLowerCase().includes(search.toLowerCase()) ||
      (f.artist || "").toLowerCase().includes(search.toLowerCase())
  );

  return (
    <main className="flex-1 flex flex-col overflow-hidden bg-[#121212] select-none">
      <div className="flex items-center justify-between p-8 pb-4 border-b border-[#222222]">
        <div className="flex items-center space-x-3">
          {selectedAlbum && (
            <button
              onClick={() => setSelectedAlbum(null)}
              className="p-1.5 rounded-lg bg-[#1E1E1E] border border-[#333333] hover:bg-[#2A2A2A] text-white transition-colors cursor-pointer mr-1"
              title={t("favorites.back")}
            >
              <ArrowLeft size={16} />
            </button>
          )}

          <div>
            <h2 className="text-2xl font-bold text-white tracking-tight flex items-center space-x-2">
              <Heart size={22} className="text-[#E5A00D]" fill="#E5A00D" />
              <span>{selectedAlbum ? selectedAlbum.title : t("favorites.title")}</span>
            </h2>
            <p className="text-xs text-[#888888] mt-0.5">
              {selectedAlbum
                ? selectedAlbum.artist
                : t("favorites.count", { count: filtered.length })}
            </p>
          </div>
        </div>

        {!selectedAlbum && (
          <div className="flex items-center space-x-4">
            <div className="relative flex items-center w-64">
              <Search size={14} className="absolute left-3 text-[#666666]" />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t("favorites.filterPlaceholder")}
                className="w-full bg-[#1A1A1A] border border-[#2B2B2B] rounded-lg pl-9 pr-3 py-1.5 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D] transition-colors"
              />
            </div>

            <div className="flex bg-[#1E1E1E] p-1 rounded-lg border border-[#333333]">
              <button
                onClick={() => {
                  setActiveTab("local");
                  setSelectedAlbum(null);
                }}
                className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                  activeTab === "local"
                    ? "bg-[#E5A00D] text-black shadow"
                    : "text-[#999999] hover:text-white"
                }`}
              >
                <HardDrive size={13} />
                <span>{t("favorites.tabLocal")}</span>
              </button>

              <button
                onClick={() => {
                  setActiveTab("plex");
                  setSelectedAlbum(null);
                }}
                className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                  activeTab === "plex"
                    ? "bg-[#E5A00D] text-black shadow"
                    : "text-[#999999] hover:text-white"
                }`}
              >
                <Server size={13} />
                <span>{t("favorites.tabPlex")}</span>
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
                {selectedAlbum.source === "plex" && selectedAlbum.plex_image ? (
                  <PlexImage
                    image={selectedAlbum.plex_image}
                    alt=""
                    className="w-full h-full object-cover"
                    fallback={<Disc3 size={64} className="text-[#444444]" />}
                  />
                ) : selectedAlbum.thumb ? (
                  <img src={selectedAlbum.thumb} alt="" className="w-full h-full object-cover" />
                ) : (
                  <Disc3 size={64} className="text-[#444444]" />
                )}
              </div>

              <div className="space-y-3">
                <span className="text-xs font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1">
                  <Heart size={13} fill="#E5A00D" />
                  <span>
                    {selectedAlbum.title} (
                    {selectedAlbum.source === "local"
                      ? t("favorites.tabLocal")
                      : t("favorites.tabPlex")}
                    )
                  </span>
                </span>
                <h1 className="text-3xl font-black text-white">{selectedAlbum.title}</h1>
                <p className="text-base text-[#CCCCCC] font-medium">{selectedAlbum.artist}</p>
                <p className="text-xs text-[#777777]">
                  {selectedAlbum.year ? `${selectedAlbum.year} • ` : ""}
                  {tracks.length} {t("favorites.tracks")}
                </p>

                <div className="pt-2 flex items-center space-x-3">
                  <button
                    onClick={() => handlePlayAll(0)}
                    disabled={!isPlaybackAvailable || tracks.length === 0}
                    className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
                  >
                    <Play size={16} fill="black" />
                    <span>{t("favorites.playAlbum")}</span>
                  </button>

                  <button
                    onClick={(e) => handleToggleFav(e, selectedAlbum)}
                    className="flex items-center space-x-2 px-4 py-2.5 rounded-xl bg-[#222222] hover:bg-[#2A2A2A] text-xs font-semibold text-[#E5A00D] border border-[#333333] transition-colors cursor-pointer"
                  >
                    <Heart size={15} fill="#E5A00D" />
                    <span>{t("favorites.removeFavorite")}</span>
                  </button>
                </div>
              </div>
            </div>

            {loadingTracks ? (
              <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                {t("plex.loading")}
              </div>
            ) : (
              <div className="bg-[#141414] border border-[#222222] rounded-xl overflow-hidden divide-y divide-[#1D1D1D]">
                <div className="grid grid-cols-12 px-4 py-2.5 text-[11px] font-bold text-[#666666] uppercase tracking-wider bg-[#181818]">
                  <span className="col-span-1 text-center">#</span>
                  <span className="col-span-8">{t("plex.tracks")}</span>
                  <span className="col-span-3 text-right flex items-center justify-end space-x-1">
                    <Clock size={12} />
                    <span>{t("player.queue")}</span>
                  </span>
                </div>

                {tracks.map((tItem, idx) => (
                  <div
                    key={tItem.uri}
                    onClick={() => handlePlayAll(idx)}
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
                        {tItem.title}
                      </span>
                      <span className="text-[11px] text-[#777777] truncate">{tItem.artist}</span>
                    </div>
                    <span className="col-span-3 text-right font-mono text-[#888888]">
                      {formatDuration(tItem.duration)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : loading ? (
          <div className="h-60 flex items-center justify-center text-xs text-[#666666]">
            {t("plex.loading")}
          </div>
        ) : filtered.length === 0 ? (
          <div className="h-60 flex flex-col items-center justify-center text-[#666666] space-y-2">
            <Heart size={36} className="opacity-30 text-[#E5A00D]" />
            <span className="text-xs">
              {activeTab === "local" ? t("favorites.emptyLocal") : t("favorites.emptyPlex")}
            </span>
            <span className="text-[11px] text-[#555555]">{t("favorites.emptyHint")}</span>
          </div>
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
            {filtered.map((fav) => {
              const isMissing = fav.source === "local" && fav.exists === false;
              return (
                <div
                  key={fav.id}
                  onClick={() => handleOpenAlbum(fav)}
                  className={`group flex flex-col cursor-pointer ${isMissing ? "opacity-60" : ""}`}
                >
                  <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md">
                    {fav.source === "plex" && fav.plex_image ? (
                      <PlexImage
                        image={fav.plex_image}
                        alt={fav.title}
                        className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                        loading="lazy"
                        fallback={
                          <div className="w-full h-full flex items-center justify-center text-[#444444]">
                            <Disc3 size={40} />
                          </div>
                        }
                      />
                    ) : fav.thumb ? (
                      <img
                        src={fav.thumb}
                        alt={fav.title}
                        className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                        loading="lazy"
                      />
                    ) : (
                      <div className="w-full h-full flex items-center justify-center text-[#444444]">
                        <Disc3 size={40} />
                      </div>
                    )}

                    <button
                      onClick={(e) => handleToggleFav(e, fav)}
                      className="absolute top-2 right-2 p-1.5 rounded-full bg-black/60 hover:bg-black/80 text-[#E5A00D] transition-transform active:scale-90 cursor-pointer shadow"
                      title={t("favorites.removeFavorite")}
                    >
                      <Heart size={14} fill="#E5A00D" />
                    </button>

                    {isMissing && (
                      <div className="absolute inset-0 bg-black/75 flex flex-col items-center justify-center p-3 text-center">
                        <AlertTriangle size={24} className="text-[#FFB020] mb-1" />
                        <span className="text-[11px] font-bold text-white">
                          {t("favorites.missingDisk")}
                        </span>
                      </div>
                    )}

                    {!isMissing && (
                      <div className="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
                        <button
                          onClick={(e) => handleQuickPlayCard(e, fav)}
                          disabled={!isPlaybackAvailable}
                          className="w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                          title={t("favorites.playAlbum")}
                        >
                          <Play size={20} className="ml-1" fill="black" />
                        </button>
                      </div>
                    )}
                  </div>

                  <span className="text-sm font-semibold text-white truncate" title={fav.title}>
                    {fav.title}
                  </span>
                  <span className="text-xs text-[#999999] truncate mt-0.5" title={fav.artist}>
                    {fav.artist}
                  </span>
                  <span className="text-[11px] text-[#666666] mt-0.5">
                    {fav.year ? `${fav.year} • ` : ""}
                    {fav.source === "local" ? t("favorites.tabLocal") : t("favorites.tabPlex")}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </main>
  );
};

export default FavoritesView;
