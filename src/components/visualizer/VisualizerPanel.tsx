import React, { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { AudioLevelFrame } from "../../types/audio";
import { SpectrumBars } from "./SpectrumBars";
import { VuHorizontal } from "./VuHorizontal";

const STORAGE_KEY = "sonante.nowPlaying.visualizerMode";
const MODES = ["combo", "vu-horizontal", "spectrum-bars"] as const;
type VisualizerMode = (typeof MODES)[number];

const isVisualizerMode = (value: string | null): value is VisualizerMode =>
  MODES.some((mode) => mode === value);

const initialMode = (): VisualizerMode => {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
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

  return (
    <section
      className={`relative mt-5 flex flex-col rounded-2xl border border-white/10 bg-black/20 px-11 py-4 sm:px-12 ${
        fullscreen ? "min-h-64 lg:min-h-80" : "min-h-44"
      }`}
      aria-label={modeLabel}
    >
      <button
        type="button"
        onClick={() => move(-1)}
        className="absolute left-2 top-1/2 z-10 -translate-y-1/2 rounded-full border border-white/10 bg-black/25 p-2 text-white/45 transition hover:border-white/25 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D]/70"
        title={t("nowPlaying.visualizer.previousMode")}
        aria-label={t("nowPlaying.visualizer.previousMode")}
      >
        <ChevronLeft size={20} />
      </button>

      <div className="mb-3 flex min-h-4 items-center justify-center gap-3 text-center text-[9px] font-bold uppercase tracking-[0.2em] text-white/40">
        <span>{modeLabel}</span>
        {!active && inactiveLabel && <span className="text-white/30">· {inactiveLabel}</span>}
      </div>

      <div className="flex min-h-0 flex-1 flex-col justify-center">
        {mode === "combo" && (
          <>
            <VuHorizontal levels={levels} active={active} />
            <div className="mt-4 border-t border-white/8 pt-3">
              <SpectrumBars
                spectrum={levels.spectrum}
                active={active}
                label={t("nowPlaying.spectrum")}
                fullscreen={fullscreen}
              />
            </div>
          </>
        )}
        {mode === "vu-horizontal" && (
          <div className={fullscreen ? "py-10" : "py-7 sm:py-8"}>
            <VuHorizontal levels={levels} active={active} large fullscreen={fullscreen} />
          </div>
        )}
        {mode === "spectrum-bars" && (
          <SpectrumBars
            spectrum={levels.spectrum}
            active={active}
            label={modeLabel}
            large
            fullscreen={fullscreen}
          />
        )}
      </div>

      <button
        type="button"
        onClick={() => move(1)}
        className="absolute right-2 top-1/2 z-10 -translate-y-1/2 rounded-full border border-white/10 bg-black/25 p-2 text-white/45 transition hover:border-white/25 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D]/70"
        title={t("nowPlaying.visualizer.nextMode")}
        aria-label={t("nowPlaying.visualizer.nextMode")}
      >
        <ChevronRight size={20} />
      </button>
    </section>
  );
};
