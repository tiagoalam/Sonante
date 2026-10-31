import {favoriteLocalTracks,favoritePlexTracks} from '../utils/favoriteTracks';
import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { PlaybackRequestNotice } from "./PlaybackRequestNotice";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useState, useRef } from "react";
import {
  Heart,
  Play,
  Disc3,
  Search,
  ArrowLeft,
  Clock,
  AlertTriangle,
  ListPlus,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { FavoriteAlbum } from "../types/favorite";
import { favoritesService } from "../services/favorites";
import { audioService } from "../services/audio";
import { plexService } from "../services/plex";
import { PlexImage } from "./PlexImage";
import { PlaylistPickerModal } from "./PlaylistPickerModal";
import { LibrarySourceTabs } from "./LibrarySourceTabs";
import type { NewPlaylistItem } from "../types/playlist";


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
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);

  const albumGeneration=useRef(0),alive=useRef(true);
  const selectedIdentity=JSON.stringify([selectedAlbum?.source,selectedAlbum?.id,selectedAlbum?.path_or_key]);
  const selectedIdentityRef=useRef(selectedIdentity);selectedIdentityRef.current=selectedIdentity;
  const closeAlbum=()=>{albumGeneration.current++;setSelectedAlbum(null);setTracks([]);setLoadingTracks(false);setPlaylistItems(null);};
  const loadFavoriteTracks=(fav:FavoriteAlbum)=>fav.source==='local'?favoriteLocalTracks(fav,audioService):plexService.getAlbumTracks(fav.path_or_key).then(items=>favoritePlexTracks(items,fav));
  const loadFavorites = async () => {
    try {
      const data = await favoritesService.getFavorites();
      if(alive.current)setFavorites(data || []);
    } catch (err) {
      if(alive.current)logError("favorites.load", err);
    } finally {
      if(alive.current)setLoading(false);
    }
  };

  useEffect(() => {
    alive.current=true;void loadFavorites();
    return()=>{alive.current=false;albumGeneration.current++;};
  }, []);

  const handleToggleFav = async (e: React.MouseEvent, fav: FavoriteAlbum) => {
    e.stopPropagation();
    try {
      await favoritesService.toggleFavorite(fav);
      if(!alive.current)return;
      setFavorites((prev) => prev.filter((item) => !(item.id === fav.id && item.source === fav.source)));
      if(selectedIdentityRef.current===JSON.stringify([fav.source,fav.id,fav.path_or_key]))closeAlbum();
      if (onFavoritesChanged) {
        onFavoritesChanged();
      }
    } catch (err) {
      logError("favorites.toggle", err);
    }
  };

  const handleOpenAlbum = async (fav: FavoriteAlbum) => {
    if(!alive.current)return;
    if (fav.source === "local" && fav.exists === false) {
      alert(t("favorites.missingDiskAlert"));
      return;
    }

    const generation=++albumGeneration.current;
    setSelectedAlbum(fav);setTracks([]);setPlaylistItems(null);setLoadingTracks(true);
    const current=()=>alive.current&&generation===albumGeneration.current;
    try {const loaded=await loadFavoriteTracks(fav);if(current())setTracks(loaded);}
    catch(err){if(current())logError("favorites.load-tracks",err);}
    finally {if(current())setLoadingTracks(false);}
  };

  const request = usePlaybackAttempt(JSON.stringify([selectedIdentity, activeTab, search, isPlaybackAvailable]));
  const handlePlayAll = (startIndex = 0) => {
    if (!isPlaybackAvailable) return;
    if (loadingTracks || !selectedAlbum || tracks.length === 0) return;
    const meta = tracks.map((t) => ({
      title: t.title,
      artist: t.artist,
      album: selectedAlbum.title,
      thumb: selectedAlbum.source === "local" ? selectedAlbum.thumb || null : null,
      plex_image: selectedAlbum.source === "plex" ? selectedAlbum.plex_image || null : null,
      uri: t.uri,
      media_locator: t.media_locator,
      duration: t.duration,
    }));
    void request.run(intent => audioService.playTracks(meta, startIndex, intent));
  };

  const handleQuickPlayCard = async (e: React.MouseEvent, fav: FavoriteAlbum) => {
    e.stopPropagation();
    if (!isPlaybackAvailable) return;
    if (fav.source === "local" && fav.exists === false) {
      alert(t("favorites.missingDiskAlert"));
      return;
    }

    await request.run(async intent => {
      const loaded=await loadFavoriteTracks(fav);
      if(!intent.current())return;
      const meta=loaded.map(track=>({...track,album:fav.title,thumb:fav.source==='local'?fav.thumb||null:null,plex_image:fav.source==='plex'?fav.plex_image||null:null}));
      if(meta.length)await audioService.playTracks(meta,0,intent);
    });
  };

  const formatDuration = (secs?: number) => {
    if (!secs || isNaN(secs)) return "--:--";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const toPlaylistItem = (track: (typeof tracks)[number]): NewPlaylistItem | null => {
    const mediaLocator = track.media_locator ?? (track.uri
      ? { kind: "local" as const, uri: track.uri }
      : null);
    if (loadingTracks || !mediaLocator || !selectedAlbum) return null;
    return {
      media_locator: mediaLocator,
      metadata: {
        title: track.title,
        artist: track.artist,
        album: selectedAlbum.title,
        duration: track.duration,
      },
    };
  };

  const albumPlaylistItems = () =>
    tracks.map(toPlaylistItem).filter((item): item is NewPlaylistItem => item !== null);

  const currentList = favorites.filter((f) => f.source === activeTab && Boolean(f.id));
  const filtered = currentList.filter(
    (f) =>
      (f.title || "").toLowerCase().includes(search.toLowerCase()) ||
      (f.artist || "").toLowerCase().includes(search.toLowerCase())
  );

  return (
    <main className="min-h-0 min-w-0 flex-1 flex flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)] select-none">
      <PlaybackRequestNotice failure={request.failure} />
      <div className="flex shrink-0 items-center justify-between p-8 pb-4 border-b border-[color:var(--theme-border-soft,#222222)]">
        <div className="flex items-center space-x-3">
          {selectedAlbum && (
            <button
              onClick={closeAlbum}
              className="p-1.5 rounded-lg bg-[var(--theme-surface-raised,#1E1E1E)] border border-[color:var(--theme-border-soft,#333333)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer mr-1"
              title={t("favorites.back")}
            >
              <ArrowLeft size={16} />
            </button>
          )}

          <div>
            <h2 className="text-2xl font-bold text-[color:var(--theme-fg-primary,#ffffff)] tracking-tight flex items-center space-x-2">
              <Heart size={22} className="text-[color:var(--theme-fg-accent,#E5A00D)]" fill="#E5A00D" />
              <span>{selectedAlbum ? selectedAlbum.title : t("favorites.title")}</span>
            </h2>
            <p className="text-xs text-[color:var(--theme-fg-muted,#888888)] mt-0.5">
              {selectedAlbum
                ? selectedAlbum.artist
                : t("favorites.count", { count: filtered.length })}
            </p>
          </div>
        </div>

        {!selectedAlbum && (
          <div className="flex items-center space-x-4">
            <div className="relative flex items-center w-64">
              <Search size={14} className="absolute left-3 text-[color:var(--theme-fg-muted,#666666)]" />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t("favorites.filterPlaceholder")}
                className="w-full bg-[var(--theme-surface-panel,#1A1A1A)] border border-[color:var(--theme-border-soft,#2B2B2B)] rounded-lg pl-9 pr-3 py-1.5 text-xs text-[color:var(--theme-fg-primary,#ffffff)] placeholder-[color:var(--theme-fg-muted,#666666)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] transition-colors"
              />
            </div>
          </div>
        )}
      </div>

      {!selectedAlbum && <LibrarySourceTabs source={activeTab} idPrefix="favorites-source"
        label={t("favorites.sourceTabs")} labels={{ local: `${t("favorites.tabLocal")}${loading ? "" : ` (${favorites.filter(item => item.source === "local" && item.id).length})`}`, plex: `${t("favorites.tabPlex")}${loading ? "" : ` (${favorites.filter(item => item.source === "plex" && item.id).length})`}` }}
        onChange={source => { setActiveTab(source); closeAlbum(); }} />}

      <div id={!selectedAlbum ? "favorites-source-panel" : undefined} role={!selectedAlbum ? "tabpanel" : undefined}
        aria-labelledby={!selectedAlbum ? `favorites-source-${activeTab}` : undefined}
        className="flex-1 overflow-y-auto p-8">
        {selectedAlbum ? (
          <div className="space-y-8 animate-in fade-in duration-100">
            <div className="flex items-end space-x-6">
              <div className="w-52 h-52 rounded-xl bg-[var(--theme-surface-raised,#202020)] border border-[color:var(--theme-border-soft,#2B2B2B)] overflow-hidden shrink-0 shadow-2xl flex items-center justify-center">
                {selectedAlbum.source === "plex" && selectedAlbum.plex_image ? (
                  <PlexImage
                    image={selectedAlbum.plex_image}
                    alt=""
                    className="w-full h-full object-cover"
                    fallback={<Disc3 size={64} className="text-[color:var(--theme-fg-faint,#444444)]" />}
                  />
                ) : selectedAlbum.thumb ? (
                  <img src={selectedAlbum.thumb} alt="" className="w-full h-full object-cover" />
                ) : (
                  <Disc3 size={64} className="text-[color:var(--theme-fg-faint,#444444)]" />
                )}
              </div>

              <div className="space-y-3">
                <span className="text-xs font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider flex items-center space-x-1">
                  <Heart size={13} fill="#E5A00D" />
                  <span>
                    {selectedAlbum.title} (
                    {selectedAlbum.source === "local"
                      ? t("favorites.tabLocal")
                      : t("favorites.tabPlex")}
                    )
                  </span>
                </span>
                <h1 className="text-3xl font-black text-[color:var(--theme-fg-primary,#ffffff)]">{selectedAlbum.title}</h1>
                <p className="text-base text-[color:var(--theme-fg-secondary,#CCCCCC)] font-medium">{selectedAlbum.artist}</p>
                <p className="text-xs text-[color:var(--theme-fg-muted,#777777)]">
                  {selectedAlbum.year ? `${selectedAlbum.year} • ` : ""}
                  {t("common.trackCount", { count: tracks.length })}
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
                    type="button"
                    onClick={() => setPlaylistItems(albumPlaylistItems())}
                    disabled={tracks.length === 0}
                    className="flex items-center space-x-2 rounded-xl border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-raised,#222222)] px-4 py-2.5 text-xs font-semibold text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] disabled:opacity-50"
                    title={t("playlists.addAlbum")}
                  >
                    <ListPlus size={15} />
                    <span>{t("playlists.addAlbum")}</span>
                  </button>

                  <button
                    onClick={(e) => handleToggleFav(e, selectedAlbum)}
                    className="flex items-center space-x-2 px-4 py-2.5 rounded-xl bg-[var(--theme-surface-raised,#222222)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] text-xs font-semibold text-[color:var(--theme-fg-accent,#E5A00D)] border border-[color:var(--theme-border-soft,#333333)] transition-colors cursor-pointer"
                  >
                    <Heart size={15} fill="#E5A00D" />
                    <span>{t("favorites.removeFavorite")}</span>
                  </button>
                </div>
              </div>
            </div>

            {loadingTracks ? (
              <div className="h-40 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
                {t("plex.loading")}
              </div>
            ) : (
              <div className="bg-[var(--theme-surface-base,#141414)] border border-[color:var(--theme-border-soft,#222222)] rounded-xl overflow-hidden divide-y divide-[color:var(--theme-border-soft,#1D1D1D)]">
                <div className="grid grid-cols-12 px-4 py-2.5 text-[11px] font-bold text-[color:var(--theme-fg-muted,#666666)] uppercase tracking-wider bg-[var(--theme-surface-panel,#181818)]">
                  <span className="col-span-1 text-center">#</span>
                  <span className="col-span-8">{t("plex.tracks")}</span>
                  <span className="col-span-3 text-right flex items-center justify-end space-x-1">
                    <Clock size={12} />
                    <span>{t("player.queue")}</span>
                  </span>
                </div>

                {tracks.map((tItem, idx) => (
                  <div
                    key={tItem.uri || (tItem.media_locator?.kind === "plex" ? tItem.media_locator.part_key : `${tItem.title}-${idx}`)}
                    onClick={() => handlePlayAll(idx)}
                    aria-disabled={!isPlaybackAvailable}
                    className={`grid grid-cols-12 px-4 py-3 text-xs items-center transition-colors group ${
                      isPlaybackAvailable
                        ? "hover:bg-[var(--theme-surface-raised,#1E1E1E)] cursor-pointer"
                        : "opacity-60 cursor-not-allowed"
                    }`}
                  >
                    <span className="col-span-1 text-center font-mono text-[color:var(--theme-fg-muted,#666666)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)]">
                      {idx + 1}
                    </span>
                    <div className="col-span-8 flex flex-col pr-2">
                      <span className="font-semibold text-[color:var(--theme-fg-primary,#ffffff)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate">
                        {tItem.title}
                      </span>
                      <span className="text-[11px] text-[color:var(--theme-fg-muted,#777777)] truncate">{tItem.artist}</span>
                    </div>
                    <div className="col-span-3 flex items-center justify-end gap-3">
                      <span className="font-mono text-[color:var(--theme-fg-muted,#888888)]">
                        {formatDuration(tItem.duration)}
                      </span>
                      <button
                        type="button"
                        onClick={(event) => {
                          event.stopPropagation();
                          const item = toPlaylistItem(tItem);
                          if (item) setPlaylistItems([item]);
                        }}
                        className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                        title={t("playlists.addTrack")}
                      >
                        <ListPlus size={15} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : loading ? (
          <div className="h-60 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
            {t("plex.loading")}
          </div>
        ) : filtered.length === 0 ? (
          <div className="h-60 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
            <Heart size={36} className="opacity-30 text-[color:var(--theme-fg-accent,#E5A00D)]" />
            <span className="text-xs">
              {activeTab === "local" ? t("favorites.emptyLocal") : t("favorites.emptyPlex")}
            </span>
            <span className="text-[11px] text-[color:var(--theme-fg-faint,#555555)]">{t("favorites.emptyHint")}</span>
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
                  <div className="relative aspect-square w-full rounded-lg bg-[var(--theme-surface-raised,#202020)] overflow-hidden mb-2.5 shadow-md">
                    {fav.source === "plex" && fav.plex_image ? (
                      <PlexImage
                        image={fav.plex_image}
                        alt={fav.title}
                        className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                        loading="lazy"
                        fallback={
                          <div className="w-full h-full flex items-center justify-center text-[color:var(--theme-fg-faint,#444444)]">
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
                      <div className="w-full h-full flex items-center justify-center text-[color:var(--theme-fg-faint,#444444)]">
                        <Disc3 size={40} />
                      </div>
                    )}
                    <button data-theme-surface="contrast"
                      onClick={(e) => handleToggleFav(e, fav)}
                      className="absolute top-2 right-2 p-1.5 rounded-full bg-black/60 hover:bg-black/80 text-[color:var(--theme-fg-accent,#E5A00D)] transition-transform active:scale-90 cursor-pointer shadow"
                      title={t("favorites.removeFavorite")}
                    >
                      <Heart size={14} fill="#E5A00D" />
                    </button>

                    {isMissing && (
                      <div data-theme-surface="contrast" className="absolute inset-0 bg-black/75 flex flex-col items-center justify-center p-3 text-center">
                        <AlertTriangle size={24} className="text-[color:var(--theme-fg-accent,#FFB020)] mb-1" />
                        <span className="text-[11px] font-bold text-[color:var(--theme-fg-primary,#ffffff)]">
                          {t("favorites.missingDisk")}
                        </span>
                      </div>
                    )}

                    {!isMissing && (
                      <div data-theme-surface="contrast" className="absolute inset-0 bg-[var(--theme-surface-subtle,rgb(0_0_0_/_0.4))] opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
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

                  <span className="text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)] truncate" title={fav.title}>
                    {fav.title}
                  </span>
                  <span className="text-xs text-[color:var(--theme-fg-secondary,#999999)] truncate mt-0.5" title={fav.artist}>
                    {fav.artist}
                  </span>
                  <span className="text-[11px] text-[color:var(--theme-fg-muted,#666666)] mt-0.5">
                    {fav.year ? `${fav.year} • ` : ""}
                    {fav.source === "local" ? t("favorites.tabLocal") : t("favorites.tabPlex")}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>
      {playlistItems && (
        <PlaylistPickerModal
          items={playlistItems}
          title={playlistItems.length === 1 ? t("playlists.addTrack") : t("playlists.addAlbum")}
          onClose={() => setPlaylistItems(null)}
        />
      )}
    </main>
  );
};

export default FavoritesView;
