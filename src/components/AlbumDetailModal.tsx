import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { X, Play, Clock, Disc3 } from "lucide-react";
import { PlexAlbum, PlexTrack } from "../types/plex";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";
import { PlexImage } from "./PlexImage";

interface Props {
  album: PlexAlbum;
  onClose: () => void;
}

export const AlbumDetailModal: React.FC<Props> = ({ album, onClose }) => {
  const { t } = useTranslation();
  const [tracks, setTracks] = useState<PlexTrack[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    plexService
      .getAlbumTracks(album.rating_key)
      .then(setTracks)
      .catch(console.error)
      .finally(() => setLoading(false));
  }, [album.rating_key]);

  const formatDuration = (ms: number) => {
    const totalSecs = Math.floor(ms / 1000);
    const mins = Math.floor(totalSecs / 60);
    const secs = totalSecs % 60;
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  };

  const playAll = (startIndex = 0) => {
    const metaTracks = tracks.map((track) => ({
      title: track.title,
      artist: album.artist,
      album: album.title,
      plex_image: track.thumb || album.thumb || null,
      media_locator: track.media_locator,
      duration: track.duration_ms ? track.duration_ms / 1000 : undefined,
    }));
    if (metaTracks.length > 0) {
      audioService.playTracks(metaTracks, startIndex);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/75 backdrop-blur-sm flex items-center justify-center p-8 animate-fade-in">
      <div className="bg-[#181818] border border-[#2B2B2B] rounded-xl w-full max-w-3xl max-h-[85vh] flex flex-col shadow-2xl overflow-hidden">
        {/* Cabeçalho do Álbum */}
        <div className="p-6 bg-[#202020] border-b border-[#2A2A2A] flex items-center justify-between">
          <div className="flex items-center space-x-5">
            <div className="w-24 h-24 rounded-lg bg-[#141414] overflow-hidden shrink-0 shadow-md">
              {album.thumb ? (
                <PlexImage image={album.thumb} alt={album.title} className="w-full h-full object-cover" />
              ) : (
                <div className="w-full h-full flex items-center justify-center text-[#444444]">
                  <Disc3 size={32} />
                </div>
              )}
            </div>
            <div>
              <h3 className="text-xl font-bold text-white tracking-tight">{album.title}</h3>
              <p className="text-sm text-[#AAAAAA] mt-0.5">{album.artist}</p>
              {album.year && (
                <p className="text-xs text-[#777777] mt-1">{album.year} • {t("plex.trackCount", { count: tracks.length })}</p>
              )}
            </div>
          </div>

          <div className="flex items-center space-x-3">
            <button
              onClick={() => playAll(0)}
              className="flex items-center space-x-2 px-4 py-2 bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs rounded-lg transition-transform active:scale-95 cursor-pointer shadow"
            >
              <Play size={16} fill="black" />
              <span>{t("plex.playAlbum")}</span>
            </button>
            <button
              onClick={onClose}
              className="p-2 text-[#888888] hover:text-white rounded-lg hover:bg-[#282828] transition-colors cursor-pointer"
            >
              <X size={20} />
            </button>
          </div>
        </div>

        {/* Lista de Faixas */}
        <div className="flex-1 overflow-y-auto p-4 space-y-1">
          {loading ? (
            <div className="py-12 text-center text-sm text-[#666666]">{t("plex.loadingTracks")}</div>
          ) : tracks.length === 0 ? (
            <div className="py-12 text-center text-sm text-[#666666]">{t("plex.noAlbumTracks")}</div>
          ) : (
            tracks.map((track, idx) => (
              <div
                key={track.rating_key}
                onClick={() => playAll(idx)}
                className="group flex items-center justify-between px-3 py-2.5 rounded-lg hover:bg-[#222222] transition-colors cursor-pointer"
              >
                <div className="flex items-center space-x-4">
                  <span className="w-6 text-center text-xs font-mono text-[#666666] group-hover:text-[#E5A00D]">
                    {track.track_index}
                  </span>
                  <span className="text-sm font-medium text-white group-hover:text-[#E5A00D] transition-colors">
                    {track.title}
                  </span>
                </div>
                <div className="flex items-center space-x-3 text-xs text-[#777777] font-mono">
                  <Clock size={12} />
		  <span>{formatDuration(track.duration_ms || 0)}</span>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
};
