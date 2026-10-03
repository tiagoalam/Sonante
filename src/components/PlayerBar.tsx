import React, { useEffect, useState, useRef } from "react";
import {
  Play,
  Pause,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
  Disc3,
  ListMusic,
  Maximize2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlexImage } from "./PlexImage";
import { MpdHealth, PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";

interface Props {
  onToggleQueue: () => void;
  isQueueOpen: boolean;
  onNavigateToArtist?: (artistName: string) => void;
  onNavigateToAlbum?: () => void;
  status: PlaybackStatus;
  health: MpdHealth;
  onOpenNowPlaying: () => void;
}

export const PlayerBar: React.FC<Props> = ({
  onToggleQueue,
  isQueueOpen,
  onNavigateToArtist,
  onNavigateToAlbum,
  status,
  health,
  onOpenNowPlaying,
}) => {
  const { t } = useTranslation();
  const [isSeeking, setIsSeeking] = useState(false);
  const [seekValue, setSeekValue] = useState(0.0);
  const [prevVolume, setPrevVolume] = useState(100);

  const isSeekingRef = useRef(false);
  useEffect(() => {
    isSeekingRef.current = isSeeking;
  }, [isSeeking]);

  const isAvailable = health.state === "available";

  useEffect(() => {
    if (isAvailable && !isSeekingRef.current) {
      setSeekValue(status.elapsed);
    }
  }, [isAvailable, status.elapsed]);

  const handlePlayToggle = async () => {
    if (!isAvailable) return;
    try {
      await audioService.togglePlay();
    } catch (err) {
      console.error("Erro no play/pause:", err);
    }
  };

  const handleNext = async () => {
    if (!isAvailable) return;
    try {
      await audioService.next();
    } catch (err) {
      console.error("Erro na faixa seguinte:", err);
    }
  };

  const handlePrevious = async () => {
    if (!isAvailable) return;
    try {
      await audioService.previous();
    } catch (err) {
      console.error("Erro na faixa anterior:", err);
    }
  };

  const handleSeekCommit = async (val: number) => {
    setIsSeeking(false);
    if (!isAvailable) return;
    try {
      await audioService.seek(val);
      setSeekValue(val);
    } catch (err) {
      console.error("Erro ao buscar posição:", err);
    }
  };

  const handleVolumeChange = async (val: number) => {
    if (!isAvailable) return;
    try {
      await audioService.setVolume(val);
    } catch (err) {
      console.error("Erro ao alterar volume:", err);
    }
  };

  const handleToggleMute = async () => {
    if (!isAvailable || !status.volume.available || !status.volume.writable) return;
    const current = status.volume.value;
    if (!status.volume.muted && current > 0) {
      setPrevVolume(current);
      await handleVolumeChange(0);
    } else {
      const restoreVolume = status.volume.muted && current > 0 ? current : prevVolume;
      await handleVolumeChange(restoreVolume > 0 ? restoreVolume : 100);
    }
  };

  const formatTime = (secs: number) => {
    if (!secs || isNaN(secs) || secs < 0) return "0:00";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const isNoTrack =
    !status.title ||
    status.state === "stop" ||
    status.title.toLowerCase().includes("nenhuma") ||
    status.title.toLowerCase().includes("no track");

  const displayTitle = isNoTrack ? t("player.noTrack") : status.title;
  const displayArtist = isNoTrack
    ? "Sonante"
    : status.artist || (status.album ? status.album : "Sonante");
  const isVolumeAvailable =
    isAvailable && status.volume.available && status.volume.writable;
  const isMuted = status.volume.muted || status.volume.value === 0;
  const volumeBackendLabel = t(`player.volumeBackend.${status.volume.backend}`);
  const volumeStatusLabel =
    isAvailable && status.volume.available && status.volume.backend !== "unavailable"
      ? `${status.volume.value}% · ${volumeBackendLabel}`
      : volumeBackendLabel;

  return (
    <footer className="h-20 bg-[#161616] border-t border-[#262626] flex items-center justify-between px-6 z-40 select-none">
      {/* 1. Metadados e Capa com Links Interativos */}
      <div className="flex items-center space-x-3.5 w-1/4 min-w-[200px]">
        <div
          onClick={() => {
            if (!isNoTrack && onNavigateToAlbum) onNavigateToAlbum();
          }}
          className={`w-12 h-12 rounded-lg bg-[#202020] border border-[#2B2B2B] overflow-hidden shrink-0 flex items-center justify-center shadow-md group/cover transition-all ${
            !isNoTrack && onNavigateToAlbum
              ? "cursor-pointer hover:border-[#E5A00D] hover:shadow-[0_0_12px_rgba(229,160,13,0.2)]"
              : ""
          }`}
          title={!isNoTrack && onNavigateToAlbum ? (status.album || displayTitle) : undefined}
        >
          {status.plex_image && !isNoTrack ? (
            <PlexImage
              image={status.plex_image}
              alt=""
              className="w-full h-full object-cover transition-transform duration-300 group-hover/cover:scale-105"
            />
          ) : status.thumb && !isNoTrack ? (
            <img
              src={status.thumb}
              alt=""
              className="w-full h-full object-cover transition-transform duration-300 group-hover/cover:scale-105"
            />
          ) : (
            <Disc3 size={24} className="text-[#444444]" />
          )}
        </div>

        <div className="flex flex-col min-w-0 pr-2">
          {!isNoTrack && onNavigateToAlbum ? (
            <button
              type="button"
              onClick={onNavigateToAlbum}
              className="text-xs font-bold text-white hover:text-[#E5A00D] transition-colors truncate text-left cursor-pointer"
              title={displayTitle}
            >
              {displayTitle}
            </button>
          ) : (
            <span className="text-xs font-bold text-white truncate" title={displayTitle}>
              {displayTitle}
            </span>
          )}

          {!isNoTrack && status.artist && onNavigateToArtist ? (
            <button
              type="button"
              onClick={() => onNavigateToArtist(status.artist)}
              className="text-[11px] text-[#888888] hover:text-[#E5A00D] transition-colors truncate mt-0.5 text-left cursor-pointer"
              title={displayArtist}
            >
              {displayArtist}
            </button>
          ) : (
            <span className="text-[11px] text-[#888888] truncate mt-0.5" title={displayArtist}>
              {displayArtist}
            </span>
          )}

          {!isAvailable && !isNoTrack && (
            <span className="text-[9px] font-semibold text-[#C9A45D] mt-1">
              {t("player.lastKnown")}
            </span>
          )}

          {isAvailable && status.audio_format && (
            <div className="flex items-center space-x-1.5 mt-1">
              <span
                className="text-[9px] font-mono px-1.5 py-0.2 rounded font-bold uppercase tracking-wider bg-[#252525] text-[#AAAAAA]"
                title={t("player.formatReportedByMpd")}
              >
                {status.audio_format}
              </span>
            </div>
          )}
        </div>
      </div>

      {/* 2. Controlos de Reprodução & Barra de Progresso */}
      <div className="flex flex-col items-center justify-center flex-1 max-w-xl px-4 space-y-1.5">
        {!isAvailable && (
          <span className="text-[10px] font-semibold text-[#C9A45D]">
            {health.state === "unavailable"
              ? t("player.engineUnavailable")
              : t("player.engineTransitioning")}
          </span>
        )}
        <div className="flex items-center space-x-5">
          <button
            onClick={handlePrevious}
            disabled={!isAvailable}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.previous")}
          >
            <SkipBack size={18} />
          </button>

          <button
            onClick={handlePlayToggle}
            disabled={!isAvailable}
            className="w-9 h-9 rounded-full bg-white hover:bg-[#E5A00D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={isAvailable && status.state === "play" ? t("player.pause") : t("player.play")}
          >
            {isAvailable && status.state === "play" ? (
              <Pause size={17} fill="black" />
            ) : (
              <Play size={17} className="ml-0.5" fill="black" />
            )}
          </button>

          <button
            onClick={handleNext}
            disabled={!isAvailable}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.next")}
          >
            <SkipForward size={18} />
          </button>
        </div>

        {/* Barra de Progresso (Seek) */}
        <div className="w-full flex items-center space-x-2.5 text-[10px] font-mono text-[#777777]">
          <span className="w-8 text-right">
            {isAvailable ? formatTime(isSeeking ? seekValue : status.elapsed) : "--:--"}
          </span>

          <input
            type="range"
            min={0}
            max={isAvailable && status.duration > 0 ? status.duration : 100}
            step={0.5}
            value={isAvailable ? (isSeeking ? seekValue : status.elapsed) : 0}
            disabled={!isAvailable || status.duration <= 0}
            onMouseDown={() => setIsSeeking(true)}
            onChange={(e) => setSeekValue(parseFloat(e.target.value))}
            onMouseUp={(e) => handleSeekCommit(parseFloat((e.target as HTMLInputElement).value))}
            className="flex-1 h-1 bg-[#262626] rounded-lg appearance-none cursor-pointer accent-[#E5A00D] disabled:opacity-30 disabled:cursor-default"
          />

          <span className="w-8 text-left">
            {isAvailable ? formatTime(status.duration) : "--:--"}
          </span>
        </div>
      </div>

      {/* 3. Volume e Botão de Gaveta de Fila */}
      <div className="flex items-center justify-end space-x-4 w-1/4 min-w-[180px]">
        <div className="flex flex-col items-center gap-1">
          <div className="flex items-center space-x-2">
            <button
              onClick={handleToggleMute}
              disabled={!isVolumeAvailable}
              className="text-[#888888] hover:text-white transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
              title={isVolumeAvailable ? (isMuted ? t("player.unmute") : t("player.mute")) : undefined}
            >
              {isVolumeAvailable && isMuted ? <VolumeX size={16} /> : <Volume2 size={16} />}
            </button>

            <input
              type="range"
              min={0}
              max={100}
              value={isVolumeAvailable ? status.volume.value : 0}
              disabled={!isVolumeAvailable}
              onChange={(e) => handleVolumeChange(parseInt(e.target.value, 10))}
              className="w-20 h-1 bg-[#262626] rounded-lg appearance-none cursor-pointer accent-[#E5A00D] disabled:opacity-35 disabled:cursor-not-allowed"
            />
          </div>

          <span className="text-[9px] leading-none text-[#777777] whitespace-nowrap">
            {volumeStatusLabel}
          </span>
        </div>

        <button
          onClick={onOpenNowPlaying}
          className="p-2 rounded-lg border border-[#262626] text-[#888888] hover:text-white hover:bg-[#202020] transition-colors cursor-pointer"
          title={t("player.openNowPlaying")}
        >
          <Maximize2 size={17} />
        </button>

        <button
          onClick={onToggleQueue}
          className={`p-2 rounded-lg border transition-colors cursor-pointer ${
            isQueueOpen
              ? "bg-[#252014] border-[#E5A00D] text-[#E5A00D]"
              : "border-[#262626] text-[#888888] hover:text-white hover:bg-[#202020]"
          }`}
          title={t("player.queue")}
        >
          <ListMusic size={17} />
        </button>
      </div>
    </footer>
  );
};

export default PlayerBar;
