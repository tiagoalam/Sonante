import { SourceAudioBadges } from "./SourceAudioBadges";
import { currentMediaKey } from "../utils/playerNavigation";
import React from "react";
import { ArrowDownLeft, ArrowUpRight, ListMusic, Maximize2, Minimize2, Pause, Play, SkipBack, SkipForward, Square, Volume2, VolumeX, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AudioLevelFrame, NowPlayingSnapshot } from "../types/audio";
import { TrackArtwork } from "./TrackArtwork";
import { requiresDeviceVolumeControl, canToggleMute, muteLabel } from "../utils/signalPath";
import { SignalPath } from "./SignalPath";
import { VisualizerPanel } from "./visualizer/VisualizerPanel";
import "./nowPlaying.css";

export interface NowPlayingPresentationProps {
  snapshot: NowPlayingSnapshot | null;
  embedded?: boolean;
  busy?: boolean;
  controlsDisabled?: boolean;
  hostingError?: boolean;
  onDetach?: () => void;
  onReincorporate?: () => void;
  onNavigate?: (view: "album" | "artist") => void;
  levels: AudioLevelFrame;
  fullscreen: boolean;
  queueOpen: boolean;
  queue: React.ReactNode;
  optionsControls?: React.ReactNode;
  analyzerAvailable: boolean;
  analyzerInactiveLabel: string;
  analyzerInactiveHelp?: string;
  controlFailed: boolean;
  isSeeking: boolean;
  seekValue: number;
  onSeekStart: () => void;
  onSeekChange: (value: number) => void;
  onSeekCommit: (value: number) => void;
  onPrevious: () => void;
  onTogglePlay: () => void;
  onNext: () => void;
  onVolume: (value: number) => void;
  onMute: () => void;
  onQueue: () => void;
  onFullscreen: () => void;
  onClose: () => void;
}
const formatTime = (seconds: number) => {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  return `${Math.floor(seconds / 60)}:${Math.floor(seconds % 60).toString().padStart(2, "0")}`;
};

// Only presentation and callbacks; window events/analyzer ownership stay in the host.
export const NowPlayingPresentation: React.FC<NowPlayingPresentationProps> = props => {
  const { t } = useTranslation();
  const status = props.snapshot?.playback;
  const health = props.snapshot?.health;
  const available = health?.state === "available";
  const playbackDiagnostic = status?.playback_error;
  const playbackNotice = playbackDiagnostic?.code === "playback_error"
    || (available && status?.state === "play"
      && (playbackDiagnostic?.code === "output_busy" || playbackDiagnostic?.code === "output_unavailable"))
    ? null : playbackDiagnostic;
  const interactive = available && !props.controlsDisabled;
  const isRadio = status?.current_media?.kind === "radio";
  const hasTrack = Boolean(status?.current_media && status.title);
  const deviceVolume = requiresDeviceVolumeControl(props.snapshot?.route, status);
  const volumeAvailable = Boolean(interactive && canToggleMute(status));
  const muted = Boolean(status?.volume.muted || status?.volume.value === 0);
  const volumeBackend = status?.volume.backend ?? "unavailable";
  const volumeLabel = available && status?.volume.available && volumeBackend !== "unavailable"
    ? `${status.volume.value}% · ${t(`player.volumeBackend.${volumeBackend}`)}` : t(`player.volumeBackend.${volumeBackend}`);
  const queueLabel = t(props.queueOpen ? "nowPlaying.hideQueue" : "nowPlaying.showQueue");
  const fullscreenLabel = t(props.fullscreen ? "nowPlaying.exitFullscreen" : "nowPlaying.enterFullscreen");
  const visualizerMessage = !available
    ? { label: t("nowPlaying.analyzerReason.mpd_unavailable"), help: t("nowPlaying.visualizer.helpUnavailable") }
    : status?.state === "stop"
      ? { label: t("nowPlaying.analyzerReason.stopped"), help: t("nowPlaying.visualizer.helpStopped") }
      : !props.analyzerAvailable
        ? { label: props.analyzerInactiveLabel, help: props.analyzerInactiveHelp ?? t("nowPlaying.visualizer.helpRetry") }
        : status?.state === "pause"
          ? { label: t("nowPlaying.visualizer.paused"), help: t("nowPlaying.visualizer.helpPaused") }
          : { label: t("nowPlaying.vuUnavailable"), help: t("nowPlaying.visualizer.helpPreparing") };
  return <main className={`np-shell text-[color:var(--theme-fg-primary,#ffffff)]${props.embedded ? " np-embedded" : ""}`} aria-busy={props.busy}>
    <div className="np-ambient" aria-hidden="true">{hasTrack && status && <TrackArtwork track={status} />}</div>
    <div className="np-layout">
      <header className="np-header">
        <div className="np-brand"><span>SONANTE</span><h1>{t("nowPlaying.title")}</h1></div>
        <div className="np-header-actions">
          <button type="button" id="now-playing-queue-toggle" onClick={props.onQueue} aria-label={queueLabel} title={queueLabel} aria-expanded={props.queueOpen} aria-controls="now-playing-queue"><ListMusic size={19} /></button>
          {props.embedded ? <button type="button" disabled={props.busy} onClick={props.onDetach} aria-label={t("nowPlaying.detach")} title={t("nowPlaying.detach")}><ArrowUpRight size={18} /></button>
            : <button type="button" disabled={props.busy || props.controlsDisabled} onClick={props.onReincorporate} aria-label={t("nowPlaying.reincorporate")} title={t("nowPlaying.reincorporate")}><ArrowDownLeft size={18} /></button>}
          {!props.embedded && <button type="button" onClick={props.onFullscreen} aria-label={fullscreenLabel} title={fullscreenLabel}><>{props.fullscreen ? <Minimize2 size={18} /> : <Maximize2 size={18} />}</></button>}
          <button type="button" autoFocus={props.embedded} disabled={props.busy && props.embedded} onClick={props.onClose} aria-label={t(props.embedded ? "nowPlaying.collapse" : "nowPlaying.close")} title={t(props.embedded ? "nowPlaying.collapse" : "nowPlaying.close")}>{props.embedded ? <ArrowDownLeft size={19} /> : <X size={19} />}</button>
        </div>
      </header>
      <div className={`np-body${props.queueOpen ? " np-with-queue" : ""}`}>
        {props.queue}
        <section className="np-media" aria-label={t("nowPlaying.title")}>
          <div className="np-cover">{status && hasTrack ? <TrackArtwork track={status} alt={status.album || status.title} /> : <TrackArtwork track={{}} />}</div>
          <div className="np-details">
            {props.hostingError && <p role="alert" className="np-error">{t("nowPlaying.transitionFailed")}</p>}
            {!status ? <p role="status">{t("nowPlaying.waitingForStatus")}</p> : <>
              <div className="np-track-heading">
                <h2 title={status.title}>{hasTrack ? status.title : t("player.noTrack")}</h2>
                <p title={status.artist}>{props.onNavigate && hasTrack && status.current_media?.kind !== "upnp" && !isRadio ? <button type="button" onClick={() => props.onNavigate?.("artist")} disabled={props.busy}>{status.artist || "Sonante"}</button> : hasTrack ? status.artist || "Sonante" : "Sonante"}</p>
                {hasTrack && status.album && <span title={status.album}>{props.onNavigate && status.current_media?.kind !== "upnp" && !isRadio ? <button type="button" onClick={() => props.onNavigate?.("album")} disabled={props.busy}>{status.album}</button> : status.album}</span>}
              </div>
                {hasTrack && available && props.snapshot?.sourceInfo && currentMediaKey(props.snapshot.sourceInfo.media) === currentMediaKey(status.current_media) && <SourceAudioBadges info={props.snapshot.sourceInfo.info} />}
              {!available && <p role="status" className="np-notice">{t(health?.state === "unavailable" ? "player.engineUnavailable" : "player.engineTransitioning")}{hasTrack && ` · ${t("player.lastKnown")}`}</p>}
              {playbackNotice && <p role="alert" className="np-error">{t(isRadio && playbackNotice.code === "format_unsupported" ? "radio.directError" : `player.playbackError.${playbackNotice.code}`)}</p>}
              {props.controlFailed && <p role="alert" className="np-error">{t("nowPlaying.controlFailed")}</p>}
              <VisualizerPanel levels={props.levels} active={Boolean(props.analyzerAvailable && available && status.state === "play")}
                inactiveLabel={visualizerMessage.label} inactiveHelp={visualizerMessage.help} fullscreen={props.fullscreen} />
              <div className="np-progress">
                <input type="range" aria-label={t("nowPlaying.seek")} min={0} max={available && status.duration > 0 ? status.duration : 100} step={0.5}
                  value={available ? props.isSeeking ? props.seekValue : status.elapsed : 0} disabled={!interactive || isRadio || status.duration <= 0}
                  onPointerDown={props.onSeekStart} onChange={event => { props.onSeekStart(); props.onSeekChange(Number(event.target.value)); }}
                  onPointerUp={event => props.onSeekCommit(Number(event.currentTarget.value))}
                  onKeyUp={event => { if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(event.key)) props.onSeekCommit(Number(event.currentTarget.value)); }} />
                <div><span>{isRadio ? t("radio.live") : available ? formatTime(props.isSeeking ? props.seekValue : status.elapsed) : "--:--"}</span><span>{isRadio ? "" : available ? formatTime(status.duration) : "--:--"}</span></div>
              </div>
              <div className="np-controls">
                <button type="button" disabled={!interactive} onClick={props.onPrevious} aria-label={t("player.previous")} title={t("player.previous")}><SkipBack size={25} /></button>
                <button type="button" disabled={!interactive} onClick={props.onTogglePlay} className="np-play" aria-label={t(status.state === "play" ? isRadio ? "radio.stop" : "player.pause" : "player.play")} title={t(status.state === "play" ? isRadio ? "radio.stop" : "player.pause" : "player.play")}>
                  {status.state === "play" ? isRadio ? <Square size={22} fill="currentColor" /> : <Pause size={25} fill="currentColor" /> : <Play size={25} fill="currentColor" className="ml-1" />}
                </button>
                <button type="button" disabled={!interactive} onClick={props.onNext} aria-label={t("player.next")} title={t("player.next")}><SkipForward size={25} /></button>
                {props.optionsControls}
              </div>
              <div className="np-volume">
                <div><button type="button" disabled={!volumeAvailable} onClick={props.onMute} aria-label={t(muteLabel(status))} title={t(muteLabel(status))}>
                  {muted ? <VolumeX size={19} /> : <Volume2 size={19} />}</button>
                  {!deviceVolume && <input type="range" aria-label={t("signalPath.volume")} min={0} max={100} value={volumeAvailable ? status.volume.value : 0} disabled={!volumeAvailable}
                    onChange={event => props.onVolume(Number(event.target.value))} />}</div>
                <span>{deviceVolume ? <>{t("player.volumeOnDevice")}</> : volumeLabel}</span>
              </div>
            </>}
          </div>
        </section>
      </div>
      {status && <footer className="np-signal"><SignalPath route={props.snapshot?.route ?? null} status={status} health={health ?? { state: "starting" }} horizontal /></footer>}
    </div>
  </main>;
};
