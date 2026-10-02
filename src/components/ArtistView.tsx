import React, { useEffect, useState } from "react";
import { ArrowLeft, Play, Pause, Disc3, ChevronDown, ChevronUp, User } from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlexAlbum, PlexTrack, SelectedArtist } from "../types/plex";
import { PlaybackStatus } from "../types/audio";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";

interface Props {
  artist: SelectedArtist;
  onBack: () => void;
  onSelectAlbum: (album: PlexAlbum) => void;
  status: PlaybackStatus;
  isPlaybackAvailable: boolean;
}

export const ArtistView: React.FC<Props> = ({
  artist,
  onBack,
  onSelectAlbum,
  status,
  isPlaybackAvailable,
}) => {
  const { t } = useTranslation();
  const [albums, setAlbums] = useState<PlexAlbum[]>([]);
  const [topTracks, setTopTracks] = useState<PlexTrack[]>([]);
  const [visibleTracksCount, setVisibleTracksCount] = useState<number>(5);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    Promise.all([
      plexService.getArtistAlbums(artist.rating_key),
      plexService.getArtistTopTracks(artist.rating_key),
    ])
      .then(([fetchedAlbums, fetchedTracks]) => {
        setAlbums(fetchedAlbums);
        setTopTracks(fetchedTracks);
      })
      .catch(console.error)
      .finally(() => setLoading(false));
  }, [artist.rating_key]);

  const formatTime = (ms: number) => {
    const totalSeconds = Math.floor(ms / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}:${seconds.toString().padStart(2, "0")}`;
  };

  const isTrackActive = (track: PlexTrack) => {
    if (!isPlaybackAvailable || status.current_media?.kind !== "plex") return false;
    return (
      track.media_locator.kind === "plex" &&
      status.current_media.server_id === track.media_locator.server_id &&
      status.current_media.part_key === track.media_locator.part_key
    );
  };

  const artistThumb = albums.find((a) => a.thumb)?.thumb;

  const handlePlayTrack = async (index: number) => {
    if (!isPlaybackAvailable) return;
    const track = topTracks[index];
    if (!track) return;

    if (isTrackActive(track)) {
      audioService.togglePlay().catch(console.error);
      return;
    }

    const metaTracks = topTracks.map((t) => ({
      title: t.title,
      artist: artist.name,
      album: t.album_title || "Single",
      thumb: t.thumb || artistThumb || null,
      media_locator: t.media_locator,
    }));

    try {
      await audioService.playTracks(metaTracks, index);
    } catch (err) {
      console.error("Falha ao tocar faixas do artista:", err);
    }
  };

  return (
    <main className="flex-1 flex flex-col overflow-y-auto bg-gradient-to-b from-[#1C1812] via-[#121212] to-[#121212] select-none p-8">
      {/* Botão de Retorno */}
      <button
        onClick={onBack}
        className="flex items-center space-x-2 text-xs font-bold text-[#888888] hover:text-[#E5A00D] transition-colors mb-6 cursor-pointer w-fit"
      >
        <ArrowLeft size={16} />
        <span>{t("artistView.back")}</span>
      </button>

      {/* Cabeçalho do Artista */}
      <div className="flex items-end space-x-6 pb-8 border-b border-[#252525]">
        <div className="w-44 h-44 rounded-full bg-[#1C1C1C] overflow-hidden shadow-2xl shrink-0 border-2 border-[#2B2B2B]">
          {artistThumb ? (
            <img src={artistThumb} alt={artist.name} className="w-full h-full object-cover" />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-[#444444]">
              <User size={64} />
            </div>
          )}
        </div>

        <div className="flex flex-col justify-end space-y-2">
          <span className="text-[11px] font-bold text-[#E5A00D] tracking-widest uppercase">
            {t("artistView.artist")}
          </span>
          <h2 className="text-4xl font-black text-white tracking-tight leading-tight">
            {artist.name}
          </h2>
          <div className="flex items-center space-x-3 text-xs text-[#777777] pt-1">
            <span>{t(albums.length === 1 ? "artistView.albumsCount_one" : "artistView.albumsCount_other", { count: albums.length })}</span>
            <span>•</span>
            <span>{t("artistView.catalogedTracks", { count: topTracks.length })}</span>
          </div>
        </div>
      </div>

      {loading ? (
        <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
          {t("artistView.loading")}
        </div>
      ) : (
        <div className="mt-8 space-y-10">
          {/* Músicas Populares */}
          {topTracks.length > 0 && (
            <div>
              <div className="flex items-center justify-between mb-4">
                <h3 className="text-lg font-bold text-white tracking-tight">
                  {t("artistView.popular")}
                </h3>
              </div>

              <div className="divide-y divide-[#1A1A1A]">
                {topTracks.slice(0, visibleTracksCount).map((track, idx) => {
                  const active = isTrackActive(track);
                  const isPlaying = active && status.state === "play";

                  return (
                    <div
                      key={track.rating_key}
                      onClick={() => handlePlayTrack(idx)}
                      aria-disabled={!isPlaybackAvailable}
                      className={`group grid grid-cols-[40px_48px_1fr_80px] items-center px-4 py-2.5 rounded-lg text-xs transition-colors ${
                        isPlaybackAvailable ? "cursor-pointer" : "cursor-not-allowed opacity-60"
                      } ${
                        active ? "bg-[#251E10] text-[#E5A00D]" : "hover:bg-[#1A1A1A] text-[#CCCCCC]"
                      }`}
                    >
                      <div className="flex items-center justify-center">
                        {isPlaying ? (
                          <div className="flex items-end space-x-0.5 h-3.5 w-3.5">
                            <span className="w-0.5 h-3 bg-[#E5A00D] animate-pulse rounded-full" />
                            <span className="w-0.5 h-1.5 bg-[#E5A00D] animate-pulse delay-75 rounded-full" />
                            <span className="w-0.5 h-3.5 bg-[#E5A00D] animate-pulse delay-150 rounded-full" />
                          </div>
                        ) : active ? (
                          <Pause size={14} className="text-[#E5A00D]" />
                        ) : (
                          <>
                            <span className="group-hover:hidden text-[#666666] font-mono">
                              {idx + 1}
                            </span>
                            <Play size={14} className="hidden group-hover:block text-white" fill="white" />
                          </>
                        )}
                      </div>

                      <div className="w-8 h-8 rounded bg-[#202020] overflow-hidden mr-3">
                        {track.thumb ? (
                          <img src={track.thumb} alt="" className="w-full h-full object-cover" />
                        ) : (
                          <div className="w-full h-full flex items-center justify-center text-[#444444]">
                            <Disc3 size={14} />
                          </div>
                        )}
                      </div>

                      <div className="flex flex-col truncate pr-4">
                        <span className={`truncate font-medium ${active ? "text-[#E5A00D] font-bold" : "text-white"}`}>
                          {track.title}
                        </span>
                        {track.album_title && (
                          <span className="text-[11px] text-[#666666] truncate mt-0.5">
                            {track.album_title}
                          </span>
                        )}
                      </div>

                      <span className={`text-right font-mono text-[11px] ${active ? "text-[#E5A00D]" : "text-[#777777]"}`}>
                        {formatTime(track.duration_ms || 0)}
                      </span>
                    </div>
                  );
                })}
              </div>

              {topTracks.length > 5 && (
                <div className="mt-3 px-4">
                  {visibleTracksCount < topTracks.length ? (
                    <button
                      onClick={() => setVisibleTracksCount((prev) => Math.min(prev + 5, topTracks.length))}
                      className="flex items-center space-x-1.5 text-xs font-bold text-[#888888] hover:text-white transition-colors cursor-pointer"
                    >
                      <span>{t("artistView.showMore")}</span>
                      <ChevronDown size={14} />
                    </button>
                  ) : (
                    <button
                      onClick={() => setVisibleTracksCount(5)}
                      className="flex items-center space-x-1.5 text-xs font-bold text-[#888888] hover:text-white transition-colors cursor-pointer"
                    >
                      <span>{t("artistView.showLess")}</span>
                      <ChevronUp size={14} />
                    </button>
                  )}
                </div>
              )}
            </div>
          )}

          {/* Discografia com Cards Clicáveis e Links Reativos */}
          <div>
            <h3 className="text-lg font-bold text-white tracking-tight mb-4">
              {t("artistView.discography")}
            </h3>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
              {albums.map((alb) => (
                <div
                  key={alb.rating_key}
                  onClick={() => onSelectAlbum(alb)}
                  className="group flex flex-col cursor-pointer"
                >
                  <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md border border-[#262626] group-hover:border-[#E5A00D]/50 transition-all">
                    {alb.thumb ? (
                      <img
                        src={alb.thumb}
                        alt={alb.title}
                        className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                        loading="lazy"
                      />
                    ) : (
                      <div className="w-full h-full flex items-center justify-center text-[#444444]">
                        <Disc3 size={40} />
                      </div>
                    )}
                  </div>
                  <span
                    className="text-sm font-semibold text-white truncate group-hover:text-[#E5A00D] transition-colors"
                    title={alb.title}
                  >
                    {alb.title}
                  </span>
                  {alb.year && (
                    <span className="text-[11px] text-[#666666] mt-0.5">{alb.year}</span>
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </main>
  );
};
