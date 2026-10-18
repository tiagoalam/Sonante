import React, { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, CircleHelp } from "lucide-react";
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
  inactiveHelp?: string;
  fullscreen: boolean;
}

export const VisualizerPanel: React.FC<VisualizerPanelProps> = ({
  levels,
  active,
  inactiveLabel,
  inactiveHelp,
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
      data-theme-surface="instrument" className="np-visualizer relative flex flex-col rounded-2xl border border-white/10 bg-[#050708]/95 px-10 py-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.045),0_16px_40px_rgba(0,0,0,0.25)]"
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

      <div className="mb-2 flex shrink-0 min-h-5 items-center justify-between gap-2 border-b border-white/[0.06] pb-2 font-mono uppercase">
        <div className="min-w-0">
          <p className="truncate text-[10px] font-semibold tracking-[0.12em] text-white/60">
            {modeLabel}
          </p>
        </div>
        <div className="flex shrink-0 items-start gap-3">
          <details className="relative font-sans text-xs font-normal normal-case tracking-normal text-white/80"
            onKeyDown={event => { if (event.key === "Escape" && event.currentTarget.open) { event.preventDefault(); event.stopPropagation(); event.currentTarget.open = false; event.currentTarget.querySelector("summary")?.focus(); } }}>
            <summary aria-label={t("nowPlaying.visualizer.howItWorks")} title={t("nowPlaying.visualizer.howItWorks")}
              className="list-none cursor-pointer rounded p-1 text-white/60 hover:text-white focus-visible:outline focus-visible:outline-[#E5A00D] [&::-webkit-details-marker]:hidden">
              <CircleHelp size={16} aria-hidden="true" />
            </summary>
            <div className="absolute right-0 top-full z-30 mt-2 max-h-[min(18rem,60vh)] w-72 max-w-[calc(100vw-6rem)] overflow-y-auto rounded-lg border border-white/15 bg-[#17191d] p-3 leading-relaxed shadow-xl">
              <p className="mb-2 font-semibold text-white">{t("nowPlaying.visualizer.howItWorks")}</p>
              <p>{t("nowPlaying.visualizer.helpOverview")}</p>
              <ul className="mt-2 list-disc space-y-1 pl-4">
                <li>{t("nowPlaying.visualizer.worksPcm")}</li>
                <li>{t("nowPlaying.visualizer.worksDsd")}</li>
                <li>{t("nowPlaying.visualizer.directDsd")}</li>
              </ul>
            </div>
          </details>
          <div className="text-right text-[8px] tracking-[0.16em] text-white/25">
            <p>{String(currentIndex).padStart(2, "0")} / {String(MODES.length).padStart(2, "0")}</p>
            <p className="mt-1">{t("nowPlaying.visualizer.measurements")}</p>
          </div>
        </div>
      </div>

      <div className="np-visualizer-graphs flex min-h-0 flex-1 flex-col justify-center">
        {!active ? <div role="status" aria-live="polite" data-visualizer-inactive className="mx-auto max-h-full w-full max-w-xl overflow-y-auto px-1 py-3 text-center">
          <p className="text-sm font-semibold leading-relaxed text-white/90">{inactiveLabel || t("nowPlaying.vuUnavailable")}</p>
          <p className="mt-2 text-xs leading-relaxed text-white/65">{inactiveHelp || t("nowPlaying.visualizer.helpRetry")}</p>
        </div> : <>
        {mode === "combo" && (
          <>
            <VuHorizontal levels={levels} active={active} />
            <div className="np-combo-spectrum min-h-0 flex-1 mt-2 border-t border-white/[0.06] pt-2">
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
        </>}
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
