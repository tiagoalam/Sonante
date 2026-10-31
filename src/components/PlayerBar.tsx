import { SourceAudioBadges } from "./SourceAudioBadges";
import type { PlayerSourceInfo } from "../types/sourceAudio";
import { currentMediaKey } from "../utils/playerNavigation";
import { PlaybackOptionsControls } from "./PlaybackOptionsControls";
import { requiresDeviceVolumeControl, canToggleMute, muteLabel, directMuteRequest, sharedMuteRequest } from "../utils/signalPath";
import { SignalPath } from "./SignalPath";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useState, useRef } from "react";
import {
  Play,
  Pause,
  Square,
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
import { ConfiguredSignalRoute, MpdHealth, PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";

interface Props {
  onToggleQueue: () => void;
  isQueueOpen: boolean;
  onNavigateToArtist?: () => void;
  onNavigateToAlbum?: () => void;
  onOpenNowPlaying?: () => void;
  status: PlaybackStatus;
  onArtworkLoaded?: (status: PlaybackStatus, image: HTMLImageElement) => void;
  onArtworkFailed?: (status: PlaybackStatus, source: string) => void;
  health: MpdHealth;
  route: ConfiguredSignalRoute | null;
  sourceInfo?: PlayerSourceInfo | null;
}

export const PlayerBar: React.FC<Props> = ({
  onToggleQueue,
  isQueueOpen,
  onNavigateToArtist,
  onNavigateToAlbum,
  onOpenNowPlaying,
  status,
  onArtworkLoaded,
  onArtworkFailed,
  health,
  route,
  sourceInfo,
}) => {
  const { t } = useTranslation();
  const [isSeeking, setIsSeeking] = useState(false);
  const [seekValue, setSeekValue] = useState(0.0);
  const [failedCover, setFailedCover] = useState<string | null>(null);

  const isSeekingRef = useRef(false);
  useEffect(() => {
    isSeekingRef.current = isSeeking;
  }, [isSeeking]);

  const isAvailable = health.state === "available";
  const isRadio = status.current_media?.kind === "radio";
  const canNavigate = !isRadio;
  const playbackDiagnostic = status.playback_error;
  const playbackNotice = playbackDiagnostic?.code === "playback_error"
    || (isAvailable && status.state === "play"
      && (playbackDiagnostic?.code === "output_busy" || playbackDiagnostic?.code === "output_unavailable"))
    ? null : playbackDiagnostic;
  const playbackNoticeText = playbackNotice
    ? t(isRadio && playbackNotice.code === "format_unsupported" ? "radio.directError" : `player.playbackError.${playbackNotice.code}`)
    : "";

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
      logError("player.toggle", err);
    }
  };

  const handleNext = async () => {
    if (!isAvailable) return;
    try {
      await audioService.next();
    } catch (err) {
      logError("player.next", err);
    }
  };

  const handlePrevious = async () => {
    if (!isAvailable) return;
    try {
      await audioService.previous();
    } catch (err) {
      logError("player.previous", err);
    }
  };

  const handleSeekCommit = async (val: number) => {
    setIsSeeking(false);
    if (!isAvailable) return;
    try {
      await audioService.seek(val);
      setSeekValue(val);
    } catch (err) {
      logError("player.seek", err);
    }
  };

  const handleVolumeChange = async (val: number) => {
    if (!isAvailable) return;
    try {
      await audioService.setVolume(val);
    } catch (err) {
      logError("player.volume", err);
    }
  };

  const handleToggleMute = async () => {
    if (!isAvailable || !canToggleMute(status)) return;
    if (status.volume.backend === "direct_equipment") {
      const request = directMuteRequest(status);
      if (request) try { await audioService.setDirectMute(request); } catch (error) { logError("player.volume", error); window.alert(t("player.directControlFailed")); }
      return;
    }
    const request=sharedMuteRequest(status);
    if(request) try {await audioService.setSharedMute(request);} catch(error){logError("player.volume",error);window.alert(t("player.sharedMuteFailed"));}
  };

  const formatTime = (secs: number) => {
    if (!secs || isNaN(secs) || secs < 0) return "0:00";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const isNoTrack =
    !status.title ||
    (status.state === "stop" && !isRadio) ||
    status.title.toLowerCase().includes("nenhuma") ||
    status.title.toLowerCase().includes("no track");

  const displayTitle = isNoTrack ? t("player.noTrack") : status.title;
  const displayArtist = isNoTrack
    ? "Sonante"
    : status.artist || (status.album ? status.album : "Sonante");
  const deviceVolume = requiresDeviceVolumeControl(route, status);
  const isVolumeAvailable =
    isAvailable && canToggleMute(status);
  const isMuted = status.volume.muted || status.volume.value === 0;
  const volumeBackendLabel = t(`player.volumeBackend.${status.volume.backend}`);
  const volumeStatusLabel =
    isAvailable && status.volume.available && status.volume.backend !== "unavailable"
      ? `${status.volume.value}% · ${volumeBackendLabel}`
      : volumeBackendLabel;

  return (
    <footer className="h-28 shrink-0 bg-[var(--theme-surface-base,#161616)] border-t border-[color:var(--theme-border-soft,#2A2A2A)] grid grid-cols-[minmax(0,1fr)_clamp(240px,26vw,400px)_minmax(0,1fr)] items-center gap-3 px-4 z-40 select-none shadow-[0_-8px_24px_rgba(0,0,0,0.16)]">
      {/* Equal side tracks keep playback at the geometric window center. */}
      <div data-player-area="left" className="flex min-w-0 items-center gap-2">
        {/* 1. Metadados e Capa com Links Interativos */}
        <div data-player-area="metadata" className="flex min-w-0 flex-1 items-center gap-2">
          <div
            onClick={() => {
              if (!isNoTrack && onNavigateToAlbum && canNavigate) onNavigateToAlbum();
            }}
            className={`w-[76px] h-[76px] rounded-xl bg-[var(--theme-surface-raised,#202020)] border border-[color:var(--theme-border-soft,#303030)] overflow-hidden shrink-0 flex items-center justify-center shadow-lg group/cover transition-all ${
              !isNoTrack && onNavigateToAlbum && canNavigate
                ? "cursor-pointer hover:border-[color:var(--theme-border-accent,#E5A00D)] hover:shadow-[0_0_12px_rgba(229,160,13,0.2)]"
                : ""
            }`}
            title={!isNoTrack && onNavigateToAlbum && canNavigate ? (status.album || displayTitle) : undefined}
          >
            {status.plex_image && !isNoTrack ? (
              <PlexImage
                image={status.plex_image}
                alt=""
                className="w-full h-full object-cover transition-transform duration-300 group-hover/cover:scale-105"
              />
            ) : status.thumb && status.thumb !== failedCover && !isNoTrack ? (
              <img
                src={status.thumb}
                ref={image => {
                  if (image?.complete && image.naturalWidth) onArtworkLoaded?.(status, image);
                }}
                onLoad={event => onArtworkLoaded?.(status, event.currentTarget)}
                onError={() => {
                  setFailedCover(status.thumb ?? null);
                  if (status.thumb) onArtworkFailed?.(status, status.thumb);
                }}
                alt=""
                className="w-full h-full object-cover transition-transform duration-300 group-hover/cover:scale-105"
              />
            ) : (
              <Disc3 size={28} className="text-[color:var(--theme-fg-faint,#444444)]" />
            )}
          </div>

          <div className="flex flex-col min-w-0 pr-2 gap-0.5">
            {!isNoTrack && onNavigateToAlbum && canNavigate ? (
              <button
                type="button"
                onClick={onNavigateToAlbum}
                className="text-sm leading-tight font-bold text-[color:var(--theme-fg-primary,#ffffff)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate text-left cursor-pointer"
                title={displayTitle}
              >
                {displayTitle}
              </button>
            ) : (
              <span className="text-sm leading-tight font-bold text-[color:var(--theme-fg-primary,#ffffff)] truncate" title={displayTitle}>
                {displayTitle}
              </span>
            )}

            {!isNoTrack && status.artist && onNavigateToArtist && canNavigate ? (
              <button
                type="button"
                onClick={onNavigateToArtist}
                className="text-xs leading-tight text-[color:var(--theme-fg-secondary,#B0B0B0)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate text-left cursor-pointer"
                title={displayArtist}
              >
                {displayArtist}
              </button>
            ) : (
              <span className="text-xs leading-tight text-[color:var(--theme-fg-secondary,#B0B0B0)] truncate" title={displayArtist}>
                {displayArtist}
              </span>
            )}

            {!isNoTrack && status.album && onNavigateToAlbum && canNavigate ? (
              <button
                type="button"
                onClick={onNavigateToAlbum}
                className="text-[10px] leading-tight text-[color:var(--theme-fg-muted,#777777)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate text-left cursor-pointer"
                title={status.album}
              >
                {status.album}
              </button>
            ) : !isNoTrack && status.album ? (
              <span className="text-[10px] leading-tight text-[color:var(--theme-fg-muted,#777777)] truncate" title={status.album}>
                {status.album}
              </span>
            ) : null}
            {!isNoTrack && isAvailable && sourceInfo && currentMediaKey(sourceInfo.media) === currentMediaKey(status.current_media) && <SourceAudioBadges info={sourceInfo.info} />}
          </div>
        </div>
      </div>

      {/* 3. Controlos de Reprodução & Barra de Progresso */}
      <div data-player-area="playback" className="flex min-w-0 flex-col items-center justify-center gap-1">
        <span
          role="status"
          aria-live="polite"
          aria-atomic="true"
          title={playbackNoticeText || undefined}
          className={playbackNotice ? "w-full truncate text-[10px] leading-tight text-[color:var(--theme-fg-accent,#C9A45D)] text-center" : "sr-only"}
        >
          {playbackNoticeText}
        </span>
        {!isAvailable && (
          <span title={health.state === "unavailable" ? t("player.engineUnavailable") : t("player.engineTransitioning")} className="w-full truncate text-center text-[10px] leading-tight font-semibold text-[color:var(--theme-fg-accent,#C9A45D)]">
            {health.state === "unavailable"
              ? t("player.engineUnavailable")
              : t("player.engineTransitioning")}
          </span>
        )}
        <div data-player-controls className="grid w-full grid-cols-[minmax(0,1fr)_18px_40px_18px_minmax(0,1fr)] items-center gap-1.5">
          <button
            onClick={handlePrevious}
            disabled={!isAvailable}
            className="col-start-2 justify-self-center text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.previous")}
          >
            <SkipBack size={18} />
          </button>

          <button
            onClick={handlePlayToggle}
            disabled={!isAvailable}
            className="col-start-3 w-10 h-10 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={isAvailable && status.state === "play" ? t(isRadio ? "radio.stop" : "player.pause") : t("player.play")}
          >
            {isAvailable && status.state === "play" ? (
              isRadio ? <Square size={15} fill="black" /> : <Pause size={17} fill="black" />
            ) : (
              <Play size={17} className="ml-0.5" fill="black" />
            )}
          </button>

          <button
            onClick={handleNext}
            disabled={!isAvailable}
            className="col-start-4 justify-self-center text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.next")}
          >
            <SkipForward size={18} />
          </button>
          <div className="col-start-5 flex min-w-0 items-center justify-start">
            <PlaybackOptionsControls options={status.options} disabled={!isAvailable || isRadio} />
          </div>
        </div>

        {/* Barra de Progresso (Seek) */}
        <div className="w-full flex items-center gap-3 text-[10px] font-mono text-[color:var(--theme-fg-muted,#888888)]">
          <span className={isRadio ? "min-w-12 whitespace-nowrap text-right" : "w-8 text-right"}>
            {isRadio ? t("radio.live") : isAvailable ? formatTime(isSeeking ? seekValue : status.elapsed) : "--:--"}
          </span>

          <input
            type="range"
            min={0}
            max={isAvailable && status.duration > 0 ? status.duration : 100}
            step={0.5}
            value={isAvailable ? (isSeeking ? seekValue : status.elapsed) : 0}
            disabled={!isAvailable || isRadio || status.duration <= 0}
            onMouseDown={() => setIsSeeking(true)}
            onChange={(e) => setSeekValue(parseFloat(e.target.value))}
            onMouseUp={(e) => handleSeekCommit(parseFloat((e.target as HTMLInputElement).value))}
            className="min-w-0 flex-1 h-1 bg-[var(--theme-surface-hover,#262626)] rounded-lg appearance-none cursor-pointer accent-[var(--theme-control-accent,#E5A00D)] disabled:opacity-30 disabled:cursor-default"
          />

          <span className="w-8 text-left">
            {isRadio ? "" : isAvailable ? formatTime(status.duration) : "--:--"}
          </span>
        </div>
      </div>

      {/* 4. Volume e ações */}
      <div data-player-area="actions" className="flex min-w-0 items-center justify-end gap-2">
        <div data-player-area="signal" className="min-w-0 max-w-[190px] flex-1">
          <SignalPath compact route={route} status={status} health={health} />
        </div>
        <div className="flex min-w-0 max-w-[176px] shrink-0 flex-col items-end gap-1.5">
          <div className="flex items-center gap-2.5">
            <button
              onClick={handleToggleMute}
              disabled={!isVolumeAvailable}
              className="text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
              aria-label={t(muteLabel(status))}
              title={isVolumeAvailable ? t(muteLabel(status)) : undefined}
            >
              {isVolumeAvailable && isMuted ? <VolumeX size={16} /> : <Volume2 size={16} />}
            </button>

            {!deviceVolume && <input
              type="range"
              min={0}
              max={100}
              value={isVolumeAvailable ? status.volume.value : 0}
              disabled={!isVolumeAvailable}
              onChange={(e) => handleVolumeChange(parseInt(e.target.value, 10))}
              className="w-24 h-1 bg-[var(--theme-surface-hover,#262626)] rounded-lg appearance-none cursor-pointer accent-[var(--theme-control-accent,#E5A00D)] disabled:opacity-35 disabled:cursor-not-allowed"
            />}
          </div>

          <span title={deviceVolume ? `${t("player.volumeOnDevice")} · ${t(muteLabel(status))}` : volumeStatusLabel} className="max-w-full whitespace-normal text-right text-[10px] leading-snug text-[color:var(--theme-fg-muted,#777777)] pr-0.5">
            {deviceVolume ? <>{t("player.volumeOnDevice")}</> : volumeStatusLabel}
          </span>
        </div>

        {onOpenNowPlaying && (
          <button
            type="button"
            onClick={onOpenNowPlaying}
            className="shrink-0 p-2.5 rounded-lg border border-[color:var(--theme-border-soft,#2D2D2D)] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#202020)] transition-colors cursor-pointer"
            title={t("player.openNowPlaying")}
          >
            <Maximize2 size={17} />
          </button>
        )}

        <button
          type="button"
          onClick={onToggleQueue}
          className={`shrink-0 p-2.5 rounded-lg border transition-colors cursor-pointer ${
            isQueueOpen
              ? "bg-[var(--theme-surface-accent,#252014)] border-[color:var(--theme-border-accent,#E5A00D)] text-[color:var(--theme-fg-accent,#E5A00D)]"
              : "border-[color:var(--theme-border-soft,#262626)] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#202020)]"
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
