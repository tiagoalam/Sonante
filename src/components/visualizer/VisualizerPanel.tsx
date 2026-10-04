import React, { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { AudioLevelFrame } from "../../types/audio";
import { AnalogVu } from "./AnalogVu";
import { SpectrumNeon } from "./SpectrumNeon";
import { SpectrumSegmented } from "./SpectrumSegmented";
import { VuHorizontal } from "./VuHorizontal";

const STORAGE_KEY = "sonante.nowPlaying.visualizerMode";
const MODES = ["combo", "spectrum-neon", "spectrum-segmented", "vu-amber", "vu-blue"] as const;
type VisualizerMode = (typeof MODES)[number];

const isVisualizerMode = (value: string | null): value is VisualizerMode =>
  MODES.some((mode) => mode === value);

const initialMode = (): VisualizerMode => {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored === "vu-horizontal") return "vu-amber";
    if (stored === "spectrum-bars") return "spectrum-neon";
    return isVisualizerMode(stored) ? stored : "combo";
  } catch {
    return "combo";
  }
};

interface VisualizerPanelProps {
  levels: AudioLevelFrame;
  active: boolean;
  inactiveLabel?: string;
  fullscreen: boolean;
}

export const VisualizerPanel: React.FC<VisualizerPanelProps> = ({
  levels,
  active,
  inactiveLabel,
  fullscreen,
}) => {
  const { t } = useTranslation();
  const [mode, setMode] = useState<VisualizerMode>(initialMode);

  useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_KEY, mode);
    } catch {
      // A escolha continua válida para a sessão quando o storage não está disponível.
    }
  }, [mode]);

  const move = (direction: -1 | 1) => {
    const current = MODES.indexOf(mode);
    setMode(MODES[(current + direction + MODES.length) % MODES.length]);
  };

  const modeLabel = t(`nowPlaying.visualizer.modes.${mode}`);
  const currentIndex = MODES.indexOf(mode) + 1;

  return (
    <section
      className={`relative flex flex-col overflow-hidden rounded-[1.35rem] border border-white/10 bg-[#050708]/95 px-11 pb-5 pt-4 shadow-[inset_0_1px_0_rgba(255,255,255,0.045),0_24px_70px_rgba(0,0,0,0.32)] sm:px-14 ${
        fullscreen ? "mt-4 min-h-0 flex-1" : "mt-5 min-h-[300px]"
      }`}
      aria-label={modeLabel}
    >
      <span className="absolute left-3 top-3 h-1.5 w-1.5 rounded-full bg-white/15 shadow-[0_0_0_1px_rgba(0,0,0,0.8)]" />
      <span className="absolute right-3 top-3 h-1.5 w-1.5 rounded-full bg-white/15 shadow-[0_0_0_1px_rgba(0,0,0,0.8)]" />
      <span className="absolute bottom-3 left-3 h-1.5 w-1.5 rounded-full bg-white/10 shadow-[0_0_0_1px_rgba(0,0,0,0.8)]" />
      <span className="absolute bottom-3 right-3 h-1.5 w-1.5 rounded-full bg-white/10 shadow-[0_0_0_1px_rgba(0,0,0,0.8)]" />
      <button
        type="button"
        onClick={() => move(-1)}
        className="absolute left-2 top-1/2 z-10 -translate-y-1/2 rounded-md border border-white/8 bg-black/30 px-1.5 py-4 text-white/35 transition hover:border-white/20 hover:bg-white/5 hover:text-white/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D]/60"
        title={t("nowPlaying.visualizer.previousMode")}
        aria-label={t("nowPlaying.visualizer.previousMode")}
      >
        <ChevronLeft size={20} />
      </button>

      <div className="mb-4 flex min-h-7 items-center justify-between gap-4 border-b border-white/[0.06] pb-3 font-mono uppercase">
        <div className="min-w-0">
          <p className="truncate text-[10px] font-semibold tracking-[0.24em] text-white/60">
            {modeLabel}
          </p>
          {!active && inactiveLabel && (
            <p className="mt-1 truncate text-[8px] tracking-[0.16em] text-white/28">
              {inactiveLabel}
            </p>
          )}
        </div>
        <div className="shrink-0 text-right text-[8px] tracking-[0.16em] text-white/25">
          <p>{String(currentIndex).padStart(2, "0")} / {String(MODES.length).padStart(2, "0")}</p>
          <p className="mt-1">{t("nowPlaying.visualizer.legend")}</p>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 flex-col justify-center">
        {mode === "combo" && (
          <>
            <VuHorizontal levels={levels} active={active} />
            <div className="mt-4 border-t border-white/[0.06] pt-4">
              <SpectrumNeon
                spectrum={levels.spectrum}
                active={active}
                label={t("nowPlaying.spectrum")}
                compact
                fullscreen={fullscreen}
              />
            </div>
          </>
        )}
        {mode === "spectrum-neon" && (
          <SpectrumNeon
            spectrum={levels.spectrum}
            active={active}
            label={modeLabel}
            fullscreen={fullscreen}
          />
        )}
        {mode === "spectrum-segmented" && (
          <SpectrumSegmented
            spectrum={levels.spectrum}
            active={active}
            label={modeLabel}
            fullscreen={fullscreen}
          />
        )}
        {(mode === "vu-amber" || mode === "vu-blue") && (
          <AnalogVu
            levels={levels}
            active={active}
            variant={mode === "vu-amber" ? "amber" : "blue"}
            leftLabel={t("nowPlaying.visualizer.left")}
            rightLabel={t("nowPlaying.visualizer.right")}
            relativeLabel={t("nowPlaying.visualizer.relativeLevel")}
            fullscreen={fullscreen}
          />
        )}
      </div>

      <button
        type="button"
        onClick={() => move(1)}
        className="absolute right-2 top-1/2 z-10 -translate-y-1/2 rounded-md border border-white/8 bg-black/30 px-1.5 py-4 text-white/35 transition hover:border-white/20 hover:bg-white/5 hover:text-white/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D]/60"
        title={t("nowPlaying.visualizer.nextMode")}
        aria-label={t("nowPlaying.visualizer.nextMode")}
      >
        <ChevronRight size={20} />
      </button>
    </section>
  );
};
