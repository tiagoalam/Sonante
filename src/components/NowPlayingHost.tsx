import { canToggleMute, directMuteRequest, sharedMuteRequest } from "../utils/signalPath";
import { PlaybackOptionsControls } from "./PlaybackOptionsControls";
import { NowPlayingPresentation } from "./NowPlayingPresentation";
import { NOW_PLAYING_LAYOUTS, NOW_PLAYING_LOOK_KEY, nowPlayingLayout, readNowPlayingLook, saveNowPlayingLook, parseNowPlayingLook, NOW_PLAYING_QUEUE_PANELS_KEY, readQueuePanels, saveQueuePanels, normalizeQueuePanels, parseQueuePanels, type NowPlayingLook } from "../utils/nowPlayingLayout";
import { NowPlayingFullscreen } from "../utils/nowPlayingFullscreen";
import { NowPlayingQueue } from "./NowPlayingQueue";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";

import { audioService } from "../services/audio";
import {
  ANALYZER_STATUS_EVENT,
  AUDIO_LEVEL_EVENT,
  startAudioAnalyzer,
  stopAudioAnalyzer,
} from "../services/nowPlayingWindow";
import { analyzerOwnerMatches } from "../utils/nowPlayingHosting";
import type { AnalyzerOwner, AnalyzerEvent, AnalyzerStatus, AudioLevelFrame, NowPlayingSnapshot } from "../types/audio";

const zeroLevels = (): AudioLevelFrame => ({
  leftRms: 0,
  rightRms: 0,
  leftPeak: 0,
  rightPeak: 0,
  spectrum: [],
});

interface Props {
  snapshot: NowPlayingSnapshot | null;
  owner: AnalyzerOwner | null;
  embedded: boolean;
  busy?: boolean;
  hostingError?: boolean;
  onReady: (owner: AnalyzerOwner) => void;
  onCollapse?: () => void;
  onDetach?: () => void;
  onReincorporate?: () => void;
  onNavigate?: (view: "album" | "artist") => void;
}
export const NowPlayingHost: React.FC<Props> = ({ snapshot, owner, embedded, busy = false, hostingError = false, onReady, onCollapse, onDetach, onReincorporate, onNavigate }) => {
  const { t } = useTranslation();
  const [queuePanels, setQueuePanels] = useState(readQueuePanels);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [isSeeking, setIsSeeking] = useState(false);
  const [seekValue, setSeekValue] = useState(0);
  const [controlFailed, setControlFailed] = useState(false);
  const [levels, setLevels] = useState<AudioLevelFrame>(zeroLevels);
  const [analyzerAvailable, setAnalyzerAvailable] = useState(false);
  const [analyzerReason, setAnalyzerReason] = useState<string | null>(null);
  const [look, setLook] = useState<NowPlayingLook>(readNowPlayingLook);
  const layout = nowPlayingLayout(look.layout);
  const queueOpen = normalizeQueuePanels(queuePanels)[layout];
  const changeQueuePanel = (open: boolean) => {
    const next = { ...normalizeQueuePanels(queuePanels), [layout]: open };
    setQueuePanels(next); saveQueuePanels(next);
  };
  const isSeekingRef = useRef(false);
  const levelTimeoutRef = useRef<number | undefined>(undefined);
  const showVisualsRef = useRef(true);
  const fullscreenController = useRef<NowPlayingFullscreen | null>(null);
  showVisualsRef.current = ["audiophile", "psycho"].includes(nowPlayingLayout(look.layout));

  const status = snapshot?.playback;
  const health = snapshot?.health;
  const isAvailable = health?.state === "available" && !busy && !!owner;
  const isVolumeAvailable = Boolean(
    isAvailable && canToggleMute(status),
  );

  useEffect(() => {
    isSeekingRef.current = isSeeking;
  }, [isSeeking]);

  useEffect(() => {
    if (status && !isSeekingRef.current) setSeekValue(status.elapsed);
  }, [status?.elapsed]);

  const readyRef = useRef(onReady);
  readyRef.current = onReady;
  useEffect(() => {
    let disposed = false;
    let startRequested = false;
    const cleanups: Array<() => void> = [];
    setLevels(zeroLevels()); setAnalyzerAvailable(false); setAnalyzerReason(null);
    if (!owner) return;
    const leaseId = window.crypto.randomUUID();
    const captureOwner = { ...owner, leaseId };
    const register = async <T,>(name: string, callback: (data: T) => void) => {
      const cleanup = await listen<AnalyzerEvent<T>>(name, event => {
        if (!disposed && analyzerOwnerMatches(captureOwner, event.payload.owner)) callback(event.payload.data);
      });
      if (disposed) cleanup(); else cleanups.push(cleanup);
    };
    void Promise.all([
      register<AudioLevelFrame>(AUDIO_LEVEL_EVENT, data => {
        // Non-animated layouts do not re-render the entire player for every analyzer frame.
        if (!showVisualsRef.current) return;
        setLevels(data);
        window.clearTimeout(levelTimeoutRef.current);
        levelTimeoutRef.current = window.setTimeout(() => setLevels(zeroLevels()), 150);
      }),
      register<AnalyzerStatus>(ANALYZER_STATUS_EVENT, data => {
        setAnalyzerAvailable(data.available); setAnalyzerReason(data.reason);
        if (!data.available) setLevels(zeroLevels());
      }),
    ]).then(() => {
      if (disposed) return;
      startRequested = true;
      // Listener readiness is separate from PCM availability (DSD/paused/error).
      // The reserved generation already prevents a second effective capture.
      const start = startAudioAnalyzer(owner, leaseId);
      readyRef.current(owner);
      return start;
    }).catch(err => {
      if (!disposed) { setAnalyzerAvailable(false); setAnalyzerReason("initial_reconcile_failed"); logError("now-playing.analyzer-start", err); }
    });
    return () => {
      disposed = true;
      window.clearTimeout(levelTimeoutRef.current);
      cleanups.forEach(cleanup => cleanup());
      if (startRequested) void stopAudioAnalyzer(owner, leaseId).catch(err => logError("now-playing.analyzer-stop", err));
    };
  }, [owner?.generation, owner?.sessionId]);
  useEffect(() => {
    const controller = new NowPlayingFullscreen({
      window: { isFullscreen: () => getCurrentWindow().isFullscreen(), setFullscreen: value => getCurrentWindow().setFullscreen(value) },
      changed: setIsFullscreen, restoreOnDispose: embedded,
      failed: (operation, error) => {
        if (operation === "status") logError("now-playing.fullscreen-status", error);
        else logError("now-playing.fullscreen-toggle", error);
      },
    });
    fullscreenController.current = controller;
    void controller.refresh();
    return () => { if (fullscreenController.current === controller) fullscreenController.current = null; controller.dispose(); };
  }, [embedded]);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || busy || event.defaultPrevented) return;
      event.preventDefault(); event.stopPropagation();
      if (queueOpen) { changeQueuePanel(false); document.getElementById("now-playing-queue-toggle")?.focus(); }
      else if (isFullscreen) void fullscreenController.current?.exit();
      else if (embedded) onCollapse?.();
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [queueOpen, queuePanels, layout, embedded, isFullscreen, busy, onCollapse]);

  useEffect(() => {
    if (status?.state !== "play" || !isAvailable || !analyzerAvailable) {
      window.clearTimeout(levelTimeoutRef.current);
      setLevels(zeroLevels());
    }
  }, [status?.state, isAvailable, analyzerAvailable]);

  useEffect(() => {
    // Both WebViews use the same origin. A handoff also reads this preference on mount.
    const syncLook = (event: StorageEvent) => {
      if (event.key === NOW_PLAYING_LOOK_KEY || event.key === null) setLook(parseNowPlayingLook(event.newValue));
      if (event.key === NOW_PLAYING_QUEUE_PANELS_KEY || event.key === null) setQueuePanels(parseQueuePanels(event.newValue));
    };
    window.addEventListener("storage", syncLook);
    return () => window.removeEventListener("storage", syncLook);
  }, []);
  const changeLook = (patch: Partial<NowPlayingLook>) => {
    const next = { ...look, ...patch };
    setLook(next); saveNowPlayingLook(next);
  };

  useEffect(() => {
    if (busy || hostingError || isSeeking || !owner || owner.host !== (embedded ? "main" : "now-playing")) return;
    const layoutShortcut = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.repeat || event.isComposing || event.keyCode === 229
        || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey || !/^[1-4]$/.test(event.key)) return;
      const root = document.querySelector(".np-shell");
      if (!root) return;
      const target = event.target as HTMLElement | null;
      if (target && typeof target.closest === "function") {
        if (target !== document.body && target !== document.documentElement && !root.contains(target)) return;
        if (target.isContentEditable || target.closest('input,textarea,select,[contenteditable]:not([contenteditable="false"]),[role="textbox"],[role="combobox"],[role="slider"],[role="spinbutton"]')) return;
      }
      if (document.querySelector('dialog[open],[aria-modal="true"],[role="dialog"],[role="menu"],[role="listbox"],details[open]')) return;
      const next = NOW_PLAYING_LAYOUTS[Number(event.key) - 1];
      event.preventDefault(); event.stopPropagation();
      if (next !== look.layout) changeLook({ layout: next });
    };
    window.addEventListener("keydown", layoutShortcut);
    return () => window.removeEventListener("keydown", layoutShortcut);
  }, [look, busy, hostingError, isSeeking, embedded, owner?.host, owner?.generation, owner?.sessionId]);

  const runControl = async (control: () => Promise<void>) => {
    if (!isAvailable) return;
    try {
      await control();
      setControlFailed(false);
    } catch (err) {
      setControlFailed(true);
      logError("now-playing.control", err);
    }
  };

  const handleSeekCommit = (value: number) => {
    setIsSeeking(false);
    void runControl(() => audioService.seek(value));
  };

  const handleToggleMute = () => {
    if (!status || !isVolumeAvailable) return;
    if (status.volume.backend === "direct_equipment") {
      const request = directMuteRequest(status);
      if (request) void runControl(() => audioService.setDirectMute(request));
      return;
    }
    const request=sharedMuteRequest(status);
    if(request) void runControl(()=>audioService.setSharedMute(request));
  };

  const handleFullscreen = () => {
    if (!busy && owner) void fullscreenController.current?.toggle();
  };

  const analyzerInactiveLabel = analyzerReason
    ? t(`nowPlaying.analyzerReason.${analyzerReason}`, {
        defaultValue: t("nowPlaying.vuUnavailable"),
      })
    : t("nowPlaying.visualizer.preparing");
  const dsdUnavailable = analyzerReason === "current_audio_format_is_dsd" || analyzerReason === "current_file_is_dsd";
  const analyzerInactiveHelp = dsdUnavailable
    ? snapshot?.route?.mode === "shared" ? t("nowPlaying.visualizer.helpDsdShared") : t("nowPlaying.visualizer.helpDsdDirect")
    : analyzerReason === "mpd_unavailable" ? t("nowPlaying.visualizer.helpUnavailable")
      : analyzerReason === "stopped" ? t("nowPlaying.visualizer.helpStopped")
        : analyzerReason ? t("nowPlaying.visualizer.helpRetry") : t("nowPlaying.visualizer.helpPreparing");

  return <NowPlayingPresentation layout={look.layout} psychoPlacement={look.psychoPlacement}
    onLayoutChange={layout => changeLook({ layout })} onPsychoPlacementChange={psychoPlacement => changeLook({ psychoPlacement })}
    snapshot={snapshot} controlsDisabled={busy || !owner} embedded={embedded} busy={busy} hostingError={hostingError}
    onDetach={onDetach} onReincorporate={onReincorporate} onNavigate={onNavigate} levels={levels} fullscreen={isFullscreen}
    optionsControls={<PlaybackOptionsControls options={status?.options ?? null} disabled={!isAvailable || status?.current_media?.kind === "radio"} />}
    queueOpen={queueOpen} queue={<NowPlayingQueue open={queueOpen} available={isAvailable} status={status} artworkContext={snapshot?.queueArtwork} detached={!embedded} />}
    analyzerAvailable={analyzerAvailable} analyzerInactiveLabel={analyzerInactiveLabel} analyzerInactiveHelp={analyzerInactiveHelp} controlFailed={controlFailed}
    isSeeking={isSeeking} seekValue={seekValue} onSeekStart={() => setIsSeeking(true)} onSeekChange={setSeekValue} onSeekCommit={handleSeekCommit}
    onPrevious={() => void runControl(audioService.previous)} onTogglePlay={() => void runControl(audioService.togglePlay)}
    onNext={() => void runControl(audioService.next)} onVolume={value => void runControl(() => audioService.setVolume(value))}
    onMute={handleToggleMute} onQueue={() => changeQueuePanel(!queueOpen)} onFullscreen={() => void handleFullscreen()}
    onClose={() => embedded ? onCollapse?.() : void getCurrentWindow().close().catch(err => logError("now-playing.close", err))} />;
};
