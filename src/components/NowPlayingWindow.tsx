import React, { useEffect, useRef, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Disc3,
  Maximize2,
  Minimize2,
  Pause,
  Play,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { audioService } from "../services/audio";
import {
  ANALYZER_STATUS_EVENT,
  AUDIO_LEVEL_EVENT,
  NOW_PLAYING_READY_EVENT,
  NOW_PLAYING_SNAPSHOT_EVENT,
  startAudioAnalyzer,
  stopAudioAnalyzer,
} from "../services/nowPlayingWindow";
import type { AnalyzerStatus, AudioLevelFrame, NowPlayingSnapshot } from "../types/audio";
import { PlexImage } from "./PlexImage";

const formatTime = (seconds: number): string => {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.floor(seconds % 60);
  return `${minutes}:${remainingSeconds.toString().padStart(2, "0")}`;
};

const SPECTRUM_BANDS = 48;
const zeroLevels = (): AudioLevelFrame => ({
  leftRms: 0,
  rightRms: 0,
  leftPeak: 0,
  rightPeak: 0,
  spectrum: [],
});

const drawSpectrum = (canvas: HTMLCanvasElement, values: number[]) => {
  const width = Math.max(1, canvas.clientWidth);
  const height = Math.max(1, canvas.clientHeight);
  const pixelRatio = window.devicePixelRatio || 1;
  const pixelWidth = Math.round(width * pixelRatio);
  const pixelHeight = Math.round(height * pixelRatio);
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  const context = canvas.getContext("2d");
  if (!context) return;
  context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
  context.clearRect(0, 0, width, height);
  const gap = Math.max(1, Math.min(3, width / 240));
  const barWidth = Math.max(1, (width - gap * (SPECTRUM_BANDS - 1)) / SPECTRUM_BANDS);
  const gradient = context.createLinearGradient(0, height, 0, 0);
  gradient.addColorStop(0, "rgba(154, 107, 5, 0.55)");
  gradient.addColorStop(1, "rgba(242, 185, 51, 0.95)");
  context.fillStyle = gradient;
  for (let index = 0; index < SPECTRUM_BANDS; index += 1) {
    const value = Math.max(0, Math.min(1, values[index] ?? 0));
    const barHeight = value * height;
    context.fillRect(index * (barWidth + gap), height - barHeight, barWidth, barHeight);
  }
};

const SpectrumCanvas: React.FC<{
  spectrum: number[];
  active: boolean;
  label: string;
}> = ({ spectrum, active, label }) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const targetRef = useRef<number[]>(Array(SPECTRUM_BANDS).fill(0));
  const displayedRef = useRef<number[]>(Array(SPECTRUM_BANDS).fill(0));
  const animationRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() => drawSpectrum(canvas, displayedRef.current));
    observer.observe(canvas);
    drawSpectrum(canvas, displayedRef.current);
    return () => {
      observer.disconnect();
      if (animationRef.current !== undefined) {
        window.cancelAnimationFrame(animationRef.current);
        animationRef.current = undefined;
      }
    };
  }, []);

  useEffect(() => {
    targetRef.current = Array.from(
      { length: SPECTRUM_BANDS },
      (_, index) => (active ? Math.max(0, Math.min(1, spectrum[index] ?? 0)) : 0),
    );
    if (animationRef.current !== undefined) return;

    const animate = () => {
      const canvas = canvasRef.current;
      if (!canvas) {
        animationRef.current = undefined;
        return;
      }
      let moving = false;
      for (let index = 0; index < SPECTRUM_BANDS; index += 1) {
        const current = displayedRef.current[index];
        const target = targetRef.current[index];
        const factor = target > current ? 0.5 : 0.16;
        const next = current + (target - current) * factor;
        if (Math.abs(target - next) > 0.001) {
          moving = true;
          displayedRef.current[index] = next;
        } else {
          displayedRef.current[index] = target;
        }
      }
      drawSpectrum(canvas, displayedRef.current);
      if (moving) {
        animationRef.current = window.requestAnimationFrame(animate);
      } else {
        animationRef.current = undefined;
      }
    };
    animationRef.current = window.requestAnimationFrame(animate);
  }, [active, spectrum]);

  return (
    <canvas
      ref={canvasRef}
      className="block h-20 w-full sm:h-24"
      role="img"
      aria-label={label}
    />
  );
};

export const NowPlayingWindow: React.FC = () => {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<NowPlayingSnapshot | null>(null);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [isSeeking, setIsSeeking] = useState(false);
  const [seekValue, setSeekValue] = useState(0);
  const [previousVolume, setPreviousVolume] = useState(100);
  const [controlFailed, setControlFailed] = useState(false);
  const [levels, setLevels] = useState<AudioLevelFrame>(zeroLevels);
  const [analyzerAvailable, setAnalyzerAvailable] = useState(false);
  const [analyzerReason, setAnalyzerReason] = useState<string | null>(null);
  const isSeekingRef = useRef(false);
  const levelTimeoutRef = useRef<number | undefined>(undefined);

  const status = snapshot?.playback;
  const health = snapshot?.health;
  const isAvailable = health?.state === "available";
  const hasTrack = Boolean(status?.current_media && status.title);
  const isVolumeAvailable = Boolean(
    isAvailable && status?.volume.available && status.volume.writable,
  );
  const isMuted = Boolean(status?.volume.muted || status?.volume.value === 0);

  useEffect(() => {
    isSeekingRef.current = isSeeking;
  }, [isSeeking]);

  useEffect(() => {
    if (status && !isSeekingRef.current) setSeekValue(status.elapsed);
  }, [status?.elapsed]);

  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];

    void Promise.all([
      listen<NowPlayingSnapshot>(NOW_PLAYING_SNAPSHOT_EVENT, (event) => {
        if (!disposed) {
          setSnapshot(event.payload);
          setControlFailed(false);
        }
      }),
      listen<AudioLevelFrame>(AUDIO_LEVEL_EVENT, (event) => {
        if (disposed) return;
        setLevels(event.payload);
        window.clearTimeout(levelTimeoutRef.current);
        levelTimeoutRef.current = window.setTimeout(() => {
          setLevels(zeroLevels());
        }, 150);
      }),
      listen<AnalyzerStatus>(ANALYZER_STATUS_EVENT, (event) => {
        if (disposed) return;
        setAnalyzerAvailable(event.payload.available);
        setAnalyzerReason(event.payload.reason);
        if (!event.payload.available) {
          setLevels(zeroLevels());
        }
      }),
    ]).then(async (listeners) => {
      if (disposed) {
        listeners.forEach((cleanup) => cleanup());
        return;
      }
      cleanups.push(...listeners);
      await emitTo("main", NOW_PLAYING_READY_EVENT);
      await startAudioAnalyzer();
    }).catch((err) => {
      if (!disposed) console.error("Falha ao iniciar análise da janela Now Playing:", err);
    });

    void getCurrentWindow()
      .isFullscreen()
      .then((value) => {
        if (!disposed) setIsFullscreen(value);
      })
      .catch((err) => {
        if (!disposed) console.error("Falha ao consultar fullscreen:", err);
      });

    return () => {
      disposed = true;
      window.clearTimeout(levelTimeoutRef.current);
      cleanups.forEach((cleanup) => cleanup());
      void stopAudioAnalyzer().catch((err) => {
        console.error("Falha ao encerrar análise da janela Now Playing:", err);
      });
    };
  }, []);

  useEffect(() => {
    if (status?.state !== "play" || !isAvailable || !analyzerAvailable) {
      window.clearTimeout(levelTimeoutRef.current);
      setLevels(zeroLevels());
    }
  }, [status?.state, isAvailable, analyzerAvailable]);

  const runControl = async (control: () => Promise<void>) => {
    if (!isAvailable) return;
    try {
      await control();
      setControlFailed(false);
    } catch (err) {
      setControlFailed(true);
      console.error("Falha em controle da janela Now Playing:", err);
    }
  };

  const handleSeekCommit = (value: number) => {
    setIsSeeking(false);
    void runControl(() => audioService.seek(value));
  };

  const handleToggleMute = () => {
    if (!status || !isVolumeAvailable) return;
    const currentVolume = status.volume.value;
    if (!isMuted && currentVolume > 0) {
      setPreviousVolume(currentVolume);
      void runControl(() => audioService.setVolume(0));
    } else {
      const restoredVolume = currentVolume > 0 ? currentVolume : previousVolume;
      void runControl(() => audioService.setVolume(restoredVolume > 0 ? restoredVolume : 100));
    }
  };

  const handleFullscreen = async () => {
    try {
      const nextValue = !(await getCurrentWindow().isFullscreen());
      await getCurrentWindow().setFullscreen(nextValue);
      setIsFullscreen(nextValue);
    } catch (err) {
      console.error("Falha ao alternar fullscreen:", err);
    }
  };

  const volumeBackend = status?.volume.backend ?? "unavailable";
  const volumeBackendLabel = t(`player.volumeBackend.${volumeBackend}`);
  const volumeStatus =
    isAvailable && status?.volume.available && volumeBackend !== "unavailable"
      ? `${status.volume.value}% · ${volumeBackendLabel}`
      : volumeBackendLabel;
  const analyzerInactiveLabel = analyzerReason
    ? t(`nowPlaying.analyzerReason.${analyzerReason}`, {
        defaultValue: analyzerReason,
      })
    : t("nowPlaying.vuUnavailable");

  return (
    <main className="relative h-screen w-screen overflow-hidden bg-[#0B0B0B] text-white select-none">
      <div className="absolute inset-0 overflow-hidden" aria-hidden="true">
        {hasTrack && status?.plex_image ? (
          <PlexImage
            image={status.plex_image}
            alt=""
            className="h-full w-full scale-110 object-cover opacity-35 blur-3xl"
          />
        ) : hasTrack && status?.thumb ? (
          <img
            src={status.thumb}
            alt=""
            className="h-full w-full scale-110 object-cover opacity-35 blur-3xl"
          />
        ) : (
          <div className="h-full w-full bg-[radial-gradient(circle_at_35%_25%,rgba(229,160,13,0.18),transparent_42%)]" />
        )}
      </div>
      <div className="absolute inset-0 bg-[linear-gradient(115deg,rgba(8,8,8,0.78),rgba(8,8,8,0.9)_58%,rgba(8,8,8,0.72))]" />

      <div className="relative z-10 flex h-full flex-col p-5 sm:p-7 lg:p-10">
        <header className="flex items-center justify-between gap-4">
          <div>
            <p className="text-[10px] font-bold uppercase tracking-[0.28em] text-[#E5A00D]">
              Sonante
            </p>
            <h1 className="mt-1 text-sm font-semibold text-white/75">
              {t("nowPlaying.title")}
            </h1>
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleFullscreen()}
              className="rounded-full border border-white/15 bg-black/25 p-2.5 text-white/75 backdrop-blur-md transition hover:border-white/30 hover:text-white"
              title={
                isFullscreen
                  ? t("nowPlaying.exitFullscreen")
                  : t("nowPlaying.enterFullscreen")
              }
            >
              {isFullscreen ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
            </button>
            <button
              type="button"
              onClick={() => {
                void getCurrentWindow().close().catch((err) => {
                  console.error("Falha ao fechar a janela Now Playing:", err);
                });
              }}
              className="rounded-full border border-white/15 bg-black/25 p-2.5 text-white/75 backdrop-blur-md transition hover:border-white/30 hover:text-white"
              title={t("nowPlaying.close")}
            >
              <X size={18} />
            </button>
          </div>
        </header>

        <section className="mx-auto grid min-h-0 w-full max-w-6xl flex-1 items-center gap-8 overflow-y-auto py-6 md:grid-cols-[minmax(260px,0.9fr)_minmax(320px,1.1fr)] lg:gap-14">
          <div className="mx-auto aspect-square w-full max-w-[min(58vh,560px)] overflow-hidden rounded-[1.75rem] border border-white/10 bg-white/5 shadow-2xl shadow-black/50">
            {hasTrack && status?.plex_image ? (
              <PlexImage
                image={status.plex_image}
                alt={status.album || status.title}
                className="h-full w-full object-cover"
                fallback={<Disc3 size={80} className="text-white/15" />}
              />
            ) : hasTrack && status?.thumb ? (
              <img
                src={status.thumb}
                alt={status.album || status.title}
                className="h-full w-full object-cover"
              />
            ) : (
              <div className="flex h-full w-full items-center justify-center">
                <Disc3 size={96} className="text-white/15" />
              </div>
            )}
          </div>

          <div className="flex min-w-0 flex-col justify-center">
            {!status ? (
              <p className="text-sm text-white/55">{t("nowPlaying.waitingForStatus")}</p>
            ) : (
              <>
                <div className="mb-4 flex flex-wrap items-center gap-2">
                  {hasTrack && status.current_media && (
                    <span className="rounded-full border border-[#E5A00D]/35 bg-[#E5A00D]/10 px-3 py-1 text-[10px] font-bold tracking-[0.18em] text-[#F2B933]">
                      {status.current_media.kind === "plex"
                        ? t("nowPlaying.sourcePlex")
                        : t("nowPlaying.sourceLocal")}
                    </span>
                  )}
                  {isAvailable && status.audio_format && (
                    <span
                      className="rounded-full border border-white/10 bg-white/5 px-3 py-1 font-mono text-[10px] font-semibold tracking-wide text-white/65"
                      title={t("player.formatReportedByMpd")}
                    >
                      {status.audio_format}
                    </span>
                  )}
                </div>

                <h2 className="line-clamp-2 text-3xl font-black leading-tight tracking-tight sm:text-4xl lg:text-5xl">
                  {hasTrack ? status.title : t("player.noTrack")}
                </h2>
                <p className="mt-3 truncate text-lg font-medium text-white/70 sm:text-xl">
                  {hasTrack ? status.artist || "Sonante" : "Sonante"}
                </p>
                {hasTrack && status.album && (
                  <p className="mt-1 truncate text-sm text-white/45 sm:text-base">{status.album}</p>
                )}

                {!isAvailable && (
                  <div className="mt-5 rounded-xl border border-amber-300/20 bg-amber-300/10 px-4 py-3 text-sm text-amber-100/80">
                    {health?.state === "unavailable"
                      ? t("player.engineUnavailable")
                      : t("player.engineTransitioning")}
                    {hasTrack && (
                      <span className="ml-2 text-amber-100/50">
                        · {t("player.lastKnown")}
                      </span>
                    )}
                  </div>
                )}
                {controlFailed && (
                  <p className="mt-4 text-sm text-red-200/80">{t("nowPlaying.controlFailed")}</p>
                )}

                <div className="mt-8">
                  <input
                    type="range"
                    min={0}
                    max={isAvailable && status.duration > 0 ? status.duration : 100}
                    step={0.5}
                    value={isAvailable ? (isSeeking ? seekValue : status.elapsed) : 0}
                    disabled={!isAvailable || status.duration <= 0}
                    onPointerDown={() => setIsSeeking(true)}
                    onChange={(event) => setSeekValue(Number(event.target.value))}
                    onPointerUp={(event) => handleSeekCommit(Number(event.currentTarget.value))}
                    className="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-white/15 accent-[#E5A00D] disabled:cursor-default disabled:opacity-35"
                  />
                  <div className="mt-2 flex justify-between font-mono text-xs text-white/45">
                    <span>{isAvailable ? formatTime(isSeeking ? seekValue : status.elapsed) : "--:--"}</span>
                    <span>{isAvailable ? formatTime(status.duration) : "--:--"}</span>
                  </div>
                </div>

                <div className="mt-5 rounded-xl border border-white/10 bg-black/20 px-4 py-3">
                  <div className="mb-2 flex items-center justify-between text-[9px] font-bold uppercase tracking-[0.2em] text-white/35">
                    <span>VU</span>
                    {!analyzerAvailable && <span>{analyzerInactiveLabel}</span>}
                  </div>
                  {(["L", "R"] as const).map((channel) => {
                    const rms = channel === "L" ? levels.leftRms : levels.rightRms;
                    const peak = channel === "L" ? levels.leftPeak : levels.rightPeak;
                    return (
                      <div key={channel} className="mt-1.5 flex items-center gap-2">
                        <span className="w-3 font-mono text-[10px] text-white/45">{channel}</span>
                        <div className="relative h-2 flex-1 overflow-hidden rounded-full bg-white/8">
                          <div
                            className="h-full rounded-full bg-gradient-to-r from-[#9A6B05] to-[#E5A00D] transition-[width] duration-100 ease-out"
                            style={{ width: `${Math.max(0, Math.min(1, rms)) * 100}%` }}
                          />
                          <span
                            className="absolute top-0 h-full w-px bg-white/80 transition-[left] duration-100 ease-out"
                            style={{
                              left: `${Math.max(0, Math.min(1, peak)) * 100}%`,
                              opacity: peak > 0 ? 1 : 0,
                            }}
                          />
                        </div>
                      </div>
                    );
                  })}
                  <div className="mt-4 border-t border-white/8 pt-3">
                    <div className="mb-2 text-[9px] font-bold uppercase tracking-[0.2em] text-white/35">
                      {t("nowPlaying.spectrum")}
                    </div>
                    <SpectrumCanvas
                      spectrum={levels.spectrum}
                      active={Boolean(
                        analyzerAvailable && isAvailable && status.state === "play",
                      )}
                      label={t("nowPlaying.spectrum")}
                    />
                  </div>
                </div>

                <div className="mt-7 flex items-center justify-center gap-7">
                  <button
                    type="button"
                    disabled={!isAvailable}
                    onClick={() => void runControl(audioService.previous)}
                    className="text-white/65 transition hover:text-white disabled:cursor-not-allowed disabled:opacity-25"
                    title={t("player.previous")}
                  >
                    <SkipBack size={26} />
                  </button>
                  <button
                    type="button"
                    disabled={!isAvailable}
                    onClick={() => void runControl(audioService.togglePlay)}
                    className="flex h-16 w-16 items-center justify-center rounded-full bg-white text-black shadow-xl transition hover:bg-[#E5A00D] active:scale-95 disabled:cursor-not-allowed disabled:opacity-25"
                    title={status.state === "play" ? t("player.pause") : t("player.play")}
                  >
                    {status.state === "play" ? (
                      <Pause size={27} fill="currentColor" />
                    ) : (
                      <Play size={27} className="ml-1" fill="currentColor" />
                    )}
                  </button>
                  <button
                    type="button"
                    disabled={!isAvailable}
                    onClick={() => void runControl(audioService.next)}
                    className="text-white/65 transition hover:text-white disabled:cursor-not-allowed disabled:opacity-25"
                    title={t("player.next")}
                  >
                    <SkipForward size={26} />
                  </button>
                </div>

                <div className="mx-auto mt-8 flex w-full max-w-sm flex-col items-center gap-2">
                  <div className="flex w-full items-center gap-3">
                    <button
                      type="button"
                      disabled={!isVolumeAvailable}
                      onClick={handleToggleMute}
                      className="text-white/60 transition hover:text-white disabled:cursor-not-allowed disabled:opacity-25"
                      title={
                        isVolumeAvailable
                          ? isMuted
                            ? t("player.unmute")
                            : t("player.mute")
                          : undefined
                      }
                    >
                      {isMuted ? <VolumeX size={20} /> : <Volume2 size={20} />}
                    </button>
                    <input
                      type="range"
                      min={0}
                      max={100}
                      value={isVolumeAvailable ? status.volume.value : 0}
                      disabled={!isVolumeAvailable}
                      onChange={(event) =>
                        void runControl(() =>
                          audioService.setVolume(Number(event.target.value)),
                        )
                      }
                      className="h-1 flex-1 cursor-pointer appearance-none rounded-full bg-white/15 accent-[#E5A00D] disabled:cursor-default disabled:opacity-35"
                    />
                  </div>
                  <span className="text-[10px] text-white/40">{volumeStatus}</span>
                </div>
              </>
            )}
          </div>
        </section>
      </div>
    </main>
  );
};
