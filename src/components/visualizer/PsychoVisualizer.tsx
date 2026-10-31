import React, { useEffect, useRef, useState } from "react";
import { Pause, Play } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AudioLevelFrame } from "../../types/audio";
import {
  drawPsychoScene, psychoCanvasSize, psychoEnergy, PsychoRenderBudget,
  PSYCHO_SCENES, psychoSceneName, readPsychoPreferences, savePsychoPreferences,
  parsePsychoPreferences, PSYCHO_PREFERENCES_KEY, type PsychoPreferences, type PsychoQuality,
} from "../../utils/psychoScene";

interface Props {
  levels: AudioLevelFrame;
  active: boolean;
  inactiveLabel: string;
  inactiveHelp?: string;
}
const silence: AudioLevelFrame = { leftRms: 0, rightRms: 0, leftPeak: 0, rightPeak: 0, spectrum: [] };

export const PsychoVisualizer: React.FC<Props> = ({ levels, active, inactiveLabel, inactiveHelp }) => {
  const { t } = useTranslation();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const frameRef = useRef(levels);
  frameRef.current = levels;
  const [frozen, setFrozen] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const [preferences, setPreferences] = useState<PsychoPreferences>(readPsychoPreferences);
  const scene = psychoSceneName(preferences.scene);
  const quality: PsychoQuality = preferences.quality === "economy" ? "economy" : "auto";
  const sceneTimeRef = useRef(0);
  const energyRef = useRef({ level: 0, bass: 0, peak: 0 });
  const budgetRef = useRef<{ quality: PsychoQuality; budget: PsychoRenderBudget } | null>(null);
  if (!budgetRef.current || budgetRef.current.quality !== quality) budgetRef.current = { quality, budget: new PsychoRenderBudget(quality) };
  const budget = budgetRef.current.budget;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    let reduced = motion.matches;
    setReducedMotion(reduced);
    let visible = !document.hidden;
    let intersecting = true;
    let raf: number | undefined;
    let previous: number | undefined;
    let resizePending = false;
    const canAnimate = () => active && !frozen && !reduced && visible && intersecting;
    const sizeCanvas = () => {
      const size = psychoCanvasSize(canvas.clientWidth, canvas.clientHeight, window.devicePixelRatio, budget.detail);
      if (canvas.width !== size.width || canvas.height !== size.height) { canvas.width = size.width; canvas.height = size.height; }
      resizePending = false;
    };
    const draw = () => {
      const target = psychoEnergy(active ? frameRef.current : silence), displayed = energyRef.current;
      displayed.level += (target.level - displayed.level) * .18;
      displayed.bass += (target.bass - displayed.bass) * .12;
      displayed.peak += (target.peak - displayed.peak) * .18;
      const started = performance.now();
      drawPsychoScene(canvas, sceneTimeRef.current, displayed, scene, budget.detail);
      return performance.now() - started;
    };
    const stop = () => { if (raf !== undefined) window.cancelAnimationFrame(raf); raf = undefined; previous = undefined; };
    const animate = (now: number) => {
      raf = undefined;
      if (!canAnimate()) return;
      const gap = previous === undefined ? 0 : now - previous;
      if (previous === undefined || gap >= budget.interval) {
        sceneTimeRef.current += Math.min(80, gap) / 1000;
        previous = now;
        if (resizePending) sizeCanvas();
        const renderMs = draw();
        if (budget.observe(renderMs, gap)) resizePending = true;
      }
      raf = window.requestAnimationFrame(animate);
    };
    const reconcile = () => {
      if (!canAnimate()) { stop(); if (visible && intersecting) draw(); }
      else if (raf === undefined) raf = window.requestAnimationFrame(animate);
    };
    const resize = () => { sizeCanvas(); if (visible && intersecting) draw(); };
    const visibility = () => { visible = !document.hidden; reconcile(); };
    const motionChanged = () => { reduced = motion.matches; setReducedMotion(reduced); reconcile(); };
    const resizeObserver = new ResizeObserver(resize);
    const visibilityObserver = new IntersectionObserver(entries => { intersecting = entries[0]?.isIntersecting ?? false; reconcile(); });
    resizeObserver.observe(canvas); visibilityObserver.observe(canvas);
    document.addEventListener("visibilitychange", visibility);
    motion.addEventListener("change", motionChanged);
    resize(); reconcile();
    return () => {
      stop(); resizeObserver.disconnect(); visibilityObserver.disconnect();
      document.removeEventListener("visibilitychange", visibility);
      motion.removeEventListener("change", motionChanged);
    };
  }, [active, frozen, scene, budget]);

  useEffect(() => {
    const sync = (event: StorageEvent) => {
      if (event.key === PSYCHO_PREFERENCES_KEY || event.key === null) setPreferences(parsePsychoPreferences(event.newValue));
    };
    window.addEventListener("storage", sync);
    return () => window.removeEventListener("storage", sync);
  }, []);
  const changePreferences = (patch: Partial<PsychoPreferences>) => {
    const next = { scene, quality, ...patch };
    setPreferences(next); savePsychoPreferences(next);
  };
  const pauseLabel = t(reducedMotion ? "nowPlaying.look.reducedMotion" : frozen ? "nowPlaying.look.resumeEffects" : "nowPlaying.look.pauseEffects");
  return <section className="np-psycho-stage" data-psycho-scene={scene} aria-label={t("nowPlaying.layouts.psycho")}>
    <canvas ref={canvasRef} aria-hidden="true" />
    <div className="np-psycho-caption"><span>{t("nowPlaying.look.psychoCaption")}</span>
      <div className="np-psycho-toolbar">
        <label><span className="sr-only">{t("nowPlaying.psycho.scene")}</span>
          <select value={scene} aria-label={t("nowPlaying.psycho.scene")} onChange={event => {
            if (PSYCHO_SCENES.some(value => value === event.target.value)) changePreferences({ scene: psychoSceneName(event.target.value) });
          }}>{PSYCHO_SCENES.map(value => <option key={value} value={value}>{t(`nowPlaying.psycho.scenes.${value}`)}</option>)}</select>
        </label>
        <label><span className="sr-only">{t("nowPlaying.psycho.quality")}</span>
          <select value={quality} aria-label={t("nowPlaying.psycho.quality")} onChange={event => {
            if (event.target.value === "auto" || event.target.value === "economy") changePreferences({ quality: event.target.value });
          }}><option value="auto">{t("nowPlaying.psycho.auto")}</option><option value="economy">{t("nowPlaying.psycho.economy")}</option></select>
        </label>
        <button type="button" onClick={() => setFrozen(value => !value)} disabled={reducedMotion || !active}
          aria-pressed={frozen} aria-label={pauseLabel} title={pauseLabel}>
          {frozen ? <Play size={14} /> : <Pause size={14} />}<span>{pauseLabel}</span>
        </button>
      </div>
    </div>
    {!active && <div className="np-psycho-inactive" role="status"><strong>{inactiveLabel}</strong>{inactiveHelp && <p>{inactiveHelp}</p>}</div>}
  </section>;
};
