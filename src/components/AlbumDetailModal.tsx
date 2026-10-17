import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { PlaybackRequestNotice } from "./PlaybackRequestNotice";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useState } from "react";
import { X, Play, Clock, Disc3, Info } from "lucide-react";
import { PlexAlbum, PlexTrack } from "../types/plex";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";
import { useTranslation } from "react-i18next";
import { PlexFileInfoModal } from "./PlexFileInfoModal";
import { PlexImage } from "./PlexImage";

interface Props {
  album: PlexAlbum;
  onClose: () => void;
}

export const AlbumDetailModal: React.FC<Props> = ({ album, onClose }) => {
  const { t } = useTranslation();
  const [tracks, setTracks] = useState<PlexTrack[]>([]);
  const [loading, setLoading] = useState(true);
  const infoAlbumKey = JSON.stringify([album.server_id ?? null, album.library_id ?? null, album.rating_key]);
  const [fileInfoTrack, setFileInfoTrack] = useState<{ albumKey: string; track: PlexTrack } | null>(null);

  useEffect(() => {
    let current = true;
    setLoading(true);
    setFileInfoTrack(null);
    plexService
      .getAlbumTracks(album.rating_key)
      .then(value => { if (current) setTracks(value); })
      .catch(error => { if (current) logError("plex.album-tracks", error); })
      .finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [album.rating_key, album.server_id, album.library_id]);

  const formatDuration = (ms: number) => {
    const totalSecs = Math.floor(ms / 1000);
    const mins = Math.floor(totalSecs / 60);
    const secs = totalSecs % 60;
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  };

  const request = usePlaybackAttempt(infoAlbumKey, "plex.album-play");
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
      void request.run(intent => audioService.playTracks(metaTracks, startIndex, intent));
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/75 backdrop-blur-sm flex items-center justify-center p-8 animate-fade-in">
      <div className="bg-[#181818] border border-[#2B2B2B] rounded-xl w-full max-w-3xl max-h-[85vh] flex flex-col shadow-2xl overflow-hidden">
        <PlaybackRequestNotice failure={request.failure} />
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
                <p className="text-xs text-[#777777] mt-1">{album.year} • {t("common.trackCount", { count: tracks.length })}</p>
              )}
            </div>
          </div>

          <div className="flex items-center space-x-3">
            <button
              onClick={() => playAll(0)}
              className="flex items-center space-x-2 px-4 py-2 bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs rounded-lg transition-transform active:scale-95 cursor-pointer shadow"
            >
              <Play size={16} fill="black" />
              <span>{t("common.playAlbum")}</span>
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
            <div className="py-12 text-center text-sm text-[#666666]">{t("common.loadingTracks")}</div>
          ) : tracks.length === 0 ? (
            <div className="py-12 text-center text-sm text-[#666666]">{t("common.noTracks")}</div>
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
                  <button type="button" data-plex-file-info onClick={event => { event.stopPropagation(); setFileInfoTrack({ albumKey: infoAlbumKey, track }); }}
                    onKeyDown={event => event.stopPropagation()} title={t("fileInfo.title")} aria-label={t("fileInfo.forTrack", { title: track.title })}
                    className="rounded-md p-1.5 hover:bg-[#2A2A2A] hover:text-[#E5A00D]"><Info size={14} aria-hidden="true" /></button>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
      {!loading && fileInfoTrack?.albumKey === infoAlbumKey && tracks.includes(fileInfoTrack.track) && <PlexFileInfoModal track={fileInfoTrack.track} onClose={() => setFileInfoTrack(null)} />}
    </div>
  );
};
