import { SleepTimerControl } from "./SleepTimerControl";
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
import "./playerBar.css";

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
  interactionDisabled?: boolean;
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
  interactionDisabled = false,
}) => {
  const { t } = useTranslation();
  const [isSeeking, setIsSeeking] = useState(false);
  const [seekValue, setSeekValue] = useState(0.0);
  const [failedCover, setFailedCover] = useState<string | null>(null);
  const [volumePreview, setVolumePreview] = useState<number | null>(null);
  const volumePreviewTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isSeekingRef = useRef(false);
  useEffect(() => {
    isSeekingRef.current = isSeeking;
  }, [isSeeking]);

  const isAvailable = health.state === "available";
  const isRadio = status.current_media?.kind === "radio";
  const canNavigate = !isRadio;
  const playbackDiagnostic = status.playback_error;
  const playbackNotice = (playbackDiagnostic?.code === "playback_error"
      && (!isAvailable || status.state === "play"))
    || (isAvailable && status.state === "play"
      && (playbackDiagnostic?.code === "output_busy" || playbackDiagnostic?.code === "output_unavailable"))
    ? null : playbackDiagnostic;
  const playbackNoticeText = playbackNotice
    ? t(status.current_media?.kind === "navidrome" && playbackNotice.code === "format_unsupported" ? "navidrome.streamError" : isRadio && playbackNotice.code === "format_unsupported" ? "radio.directError" : `player.playbackError.${playbackNotice.code}`)
    : "";

  useEffect(() => {
    if (isAvailable && !isSeekingRef.current) {
      setSeekValue(status.elapsed);
    }
  }, [isAvailable, status.elapsed]);

  useEffect(() => {
    if (volumePreview !== null && status.volume.available && status.volume.value === volumePreview) {
      setVolumePreview(null);
      if (volumePreviewTimer.current) clearTimeout(volumePreviewTimer.current);
      volumePreviewTimer.current = null;
    }
  }, [status.volume.available, status.volume.value, volumePreview]);

  useEffect(() => () => {
    if (volumePreviewTimer.current) clearTimeout(volumePreviewTimer.current);
  }, []);

  const handlePlayToggle = async () => {
    if (interactionDisabled || !isAvailable) return;
    try {
      await audioService.togglePlay();
    } catch (err) {
      logError("player.toggle", err);
    }
  };

  const handleNext = async () => {
    if (interactionDisabled || !isAvailable) return;
    try {
      await audioService.next();
    } catch (err) {
      logError("player.next", err);
    }
  };

  const handlePrevious = async () => {
    if (interactionDisabled || !isAvailable) return;
    try {
      await audioService.previous();
    } catch (err) {
      logError("player.previous", err);
    }
  };

  const handleSeekCommit = async (val: number) => {
    setIsSeeking(false);
    if (interactionDisabled || !isAvailable) return;
    try {
      await audioService.seek(val);
      setSeekValue(val);
    } catch (err) {
      logError("player.seek", err);
    }
  };

  const handleVolumeChange = async (val: number) => {
    if (interactionDisabled || !isAvailable) return;
    setVolumePreview(val);
    if (volumePreviewTimer.current) clearTimeout(volumePreviewTimer.current);
    volumePreviewTimer.current = setTimeout(() => {
      setVolumePreview(null);
      volumePreviewTimer.current = null;
    }, 1200);
    try {
      await audioService.setVolume(val);
    } catch (err) {
      setVolumePreview(null);
      if (volumePreviewTimer.current) clearTimeout(volumePreviewTimer.current);
      volumePreviewTimer.current = null;
      logError("player.volume", err);
    }
  };

  const handleToggleMute = async () => {
    if (interactionDisabled || !isAvailable || !canToggleMute(status)) return;
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
    isAvailable && status.volume.available && canToggleMute(status);
  const isMuted = status.volume.muted || status.volume.value === 0;
  const displayedVolume = volumePreview ?? status.volume.value;
  const volumeBackendLabel = t(`player.volumeBackend.${status.volume.backend}`);
  const volumeStatusLabel =
    isAvailable && status.volume.available && status.volume.backend !== "unavailable"
      ? `${status.volume.value}% · ${volumeBackendLabel}`
      : volumeBackendLabel;
  const compactVolumeLabel = deviceVolume
    ? t("player.volumeOnDeviceShort")
    : t(`player.volumeBackendCompact.${isAvailable && status.volume.available ? status.volume.backend : "unavailable"}`);
  const compactVolumeTitle = deviceVolume
    ? `${t("player.volumeOnDevice")} · ${t(muteLabel(status))}`
    : `${isAvailable && status.volume.available ? volumeStatusLabel : t("player.volumeBackend.unavailable")} · ${t(muteLabel(status))}`;

  return (
    <footer className="player-compact-grid shrink-0 bg-[var(--theme-surface-base,#161616)] border-t border-[color:var(--theme-border-soft,#2A2A2A)] z-40 select-none shadow-[0_-8px_24px_rgba(0,0,0,0.16)]">
        <div data-player-area="metadata" className="flex min-w-0 items-center gap-2">
          <div
            data-player-cover
            onClick={() => {
              if (!interactionDisabled && !isNoTrack && onNavigateToAlbum && canNavigate) onNavigateToAlbum();
            }}
            className={`w-[76px] h-[76px] rounded-xl bg-[var(--theme-surface-raised,#202020)] border border-[color:var(--theme-border-soft,#303030)] overflow-hidden shrink-0 flex items-center justify-center shadow-lg group/cover transition-all ${
              !interactionDisabled && !isNoTrack && onNavigateToAlbum && canNavigate
                ? "cursor-pointer hover:border-[color:var(--theme-border-accent,#E5A00D)] hover:shadow-[0_0_12px_rgba(229,160,13,0.2)]"
                : ""
            }`}
            title={!interactionDisabled && !isNoTrack && onNavigateToAlbum && canNavigate ? (status.album || displayTitle) : undefined}
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

          <div data-player-metadata-copy className="flex flex-col min-w-0 pr-2 gap-0.5">
            {!interactionDisabled && !isNoTrack && onNavigateToAlbum && canNavigate ? (
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

            {!interactionDisabled && !isNoTrack && status.artist && onNavigateToArtist && canNavigate ? (
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

            {!interactionDisabled && !isNoTrack && status.album && onNavigateToAlbum && canNavigate ? (
              <button
                type="button"
                onClick={onNavigateToAlbum}
                className="text-[10px] leading-tight text-[color:var(--theme-fg-muted,#777777)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate text-left cursor-pointer"
                data-player-album
                title={status.album}
              >
                {status.album}
              </button>
            ) : !isNoTrack && status.album ? (
              <span data-player-album className="text-[10px] leading-tight text-[color:var(--theme-fg-muted,#777777)] truncate" title={status.album}>
                {status.album}
              </span>
            ) : null}
            {!isNoTrack && isAvailable && sourceInfo && currentMediaKey(sourceInfo.media) === currentMediaKey(status.current_media) && <span data-player-source-badges><SourceAudioBadges info={sourceInfo.info} /></span>}
          </div>
        </div>

      <div data-player-area="transport" className="flex min-w-0 flex-col items-center justify-center gap-1">
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
            disabled={interactionDisabled || !isAvailable}
            className="col-start-2 justify-self-center text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.previous")}
          >
            <SkipBack size={18} />
          </button>

          <button
            onClick={handlePlayToggle}
            disabled={interactionDisabled || !isAvailable}
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
            disabled={interactionDisabled || !isAvailable}
            className="col-start-4 justify-self-center text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.next")}
          >
            <SkipForward size={18} />
          </button>
          <div className="col-start-5 flex min-w-0 items-center justify-start">
            <PlaybackOptionsControls options={status.options} disabled={interactionDisabled || !isAvailable || isRadio} />
          </div>
        </div>

        {/* Barra de Progresso (Seek) */}
        <div className="w-full flex items-center gap-3 text-[10px] font-mono text-[color:var(--theme-fg-muted,#888888)]">
          <span className={isRadio ? "min-w-12 whitespace-nowrap text-right" : "w-8 text-right"}>
            {isRadio ? t("radio.live") : isAvailable ? formatTime(isSeeking ? seekValue : status.elapsed) : "--:--"}
          </span>

          <input
            data-player-seek
            type="range"
            min={0}
            max={isAvailable && status.duration > 0 ? status.duration : 100}
            step={0.5}
            value={isAvailable ? (isSeeking ? seekValue : status.elapsed) : 0}
            disabled={interactionDisabled || !isAvailable || isRadio || status.duration <= 0}
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

      <div data-player-area="state-volume" className="player-compact-state-volume">
        <div data-player-area="signal" className="min-w-0">
          <SignalPath compact route={route} status={status} health={health} />
        </div>
        <div data-player-volume-controls className={`player-compact-volume-controls min-w-0 items-center justify-center ${!deviceVolume && isVolumeAvailable ? "has-slider" : "external"}`}>
            <button
              onClick={handleToggleMute}
              disabled={interactionDisabled || !isVolumeAvailable}
              className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-[color:var(--theme-border-soft,#2D2D2D)] text-[color:var(--theme-fg-muted,#888888)] transition-colors hover:bg-[var(--theme-surface-raised,#202020)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)] cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
              aria-label={t(muteLabel(status))}
              title={isVolumeAvailable ? t(muteLabel(status)) : undefined}
            >
              {isVolumeAvailable && isMuted ? <VolumeX size={16} /> : <Volume2 size={16} />}
            </button>

            {deviceVolume ? <span data-player-volume-status title={compactVolumeTitle} className="inline-flex h-6 max-w-[110px] min-w-0 items-center truncate rounded-md border border-[color:var(--theme-border-soft,#2D2D2D)] bg-[var(--theme-surface-raised,#202020)] px-1.5 text-[10px] font-medium leading-none text-[color:var(--theme-fg-secondary,#B3B3B3)]">
              {compactVolumeLabel}
            </span> : isVolumeAvailable ? <>
              <input
                data-player-volume-slider
                type="range"
                min={0}
                max={100}
                value={displayedVolume}
                disabled={interactionDisabled}
                onChange={(e) => handleVolumeChange(parseInt(e.target.value, 10))}
                aria-label={t("player.volume")}
                title={compactVolumeTitle}
                className="player-compact-volume-slider h-1 min-w-0 bg-[var(--theme-surface-hover,#262626)] rounded-lg appearance-none cursor-pointer accent-[var(--theme-control-accent,#E5A00D)] disabled:opacity-35 disabled:cursor-not-allowed"
              />
              <output data-player-volume-percent aria-live="off" className="player-compact-volume-percent" aria-label={t("player.volume")}>{Math.round(displayedVolume)}%</output>
            </> : <span data-player-volume-status title={compactVolumeTitle} className="inline-flex h-6 max-w-[110px] min-w-0 items-center truncate rounded-md border border-[color:var(--theme-border-soft,#2D2D2D)] bg-[var(--theme-surface-raised,#202020)] px-1.5 text-[10px] font-medium leading-none text-[color:var(--theme-fg-secondary,#B3B3B3)]">
              {compactVolumeLabel}
            </span>}
        </div>
      </div>

      <div data-player-area="actions" data-player-timer-actions className="player-compact-actions">
        {onOpenNowPlaying && (
          <button
            data-player-action="expand"
            type="button"
            onClick={onOpenNowPlaying}
            disabled={interactionDisabled}
            className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-[color:var(--theme-border-soft,#2D2D2D)] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#202020)] transition-colors cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed"
            title={t("player.openNowPlaying")}
          >
            <Maximize2 size={17} />
          </button>
        )}
        <button
          data-player-action="queue"
          type="button"
          onClick={onToggleQueue}
          disabled={interactionDisabled}
          className={`inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border transition-colors cursor-pointer ${
            isQueueOpen
              ? "bg-[var(--theme-surface-accent,#252014)] border-[color:var(--theme-border-accent,#E5A00D)] text-[color:var(--theme-fg-accent,#E5A00D)]"
              : "border-[color:var(--theme-border-soft,#262626)] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#202020)]"
          }`}
          title={t("player.queue")}
        >
          <ListMusic size={17} />
        </button>
        <SleepTimerControl compact disabled={interactionDisabled} />
      </div>
    </footer>
  );
};

export default PlayerBar;
