import React, { useEffect, useRef } from "react";
import type { AudioLevelFrame } from "../../types/audio";
import { InstrumentMotion } from "../../utils/instrumentMotion";
import { drawInstrumentBackground, drawInstrumentForeground, instrumentCanvasSize, type InstrumentMode, type InstrumentLabels } from "../../utils/instrumentDrawing";

interface Props {
  levels: AudioLevelFrame;
  active: boolean;
  mode: InstrumentMode;
  label: string;
  labels?: InstrumentLabels;
  fullscreen?: boolean;
  compact?: boolean;
}
const defaultLabels: InstrumentLabels = { left: "L", right: "R", relative: "RMS · dBFS" };

// A whole instrument (including combo) has one canvas, target and animation clock.
export const InstrumentCanvas: React.FC<Props> = ({ levels, active, mode, label, labels = defaultLabels, fullscreen = false, compact = false }) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const modelRef = useRef<InstrumentMotion | null>(null);
  if (!modelRef.current) modelRef.current = new InstrumentMotion();
  const model = modelRef.current;
  const viewRef = useRef({ mode, labels });
  const wakeRef = useRef<() => void>(() => {});

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let disposed = false, visible = !document.hidden, intersecting = true;
    let raf: number | undefined, previous: number | undefined;
    let size = instrumentCanvasSize(canvas.clientWidth, canvas.clientHeight, window.devicePixelRatio);
    let face: HTMLCanvasElement | null = null, faceKey = "";
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    let reduced = motion.matches;
    const draw = () => {
      const ctx = canvas.getContext("2d", { alpha: false });
      if (!ctx) return;
      if (canvas.width !== size.width || canvas.height !== size.height) { canvas.width = size.width; canvas.height = size.height; }
      const view = viewRef.current, key = JSON.stringify([size.width, size.height, view.mode, view.labels]);
      if (!face || faceKey !== key) {
        face = document.createElement("canvas"); face.width = size.width; face.height = size.height;
        const base = face.getContext("2d", { alpha: false });
        if (!base) { face = null; return; }
        base.setTransform(size.ratio, 0, 0, size.ratio, 0, 0);
        drawInstrumentBackground(base, size.logicalWidth, size.logicalHeight, view.mode, view.labels);
        faceKey = key;
      }
      ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.drawImage(face, 0, 0);
      ctx.setTransform(size.ratio, 0, 0, size.ratio, 0, 0);
      drawInstrumentForeground(ctx, size.logicalWidth, size.logicalHeight, view.mode, model.frame);
    };
    const stop = () => { if (raf !== undefined) window.cancelAnimationFrame(raf); raf = undefined; previous = undefined; };
    const tick = (time: number) => {
      raf = undefined;
      if (disposed || !visible || !intersecting) return;
      const dt = previous === undefined ? 1000 / 60 : Math.max(0, time - previous);
      if (previous !== undefined && dt < 1000 / 60 - .01) { raf = window.requestAnimationFrame(tick); return; }
      previous = time;
      const moving = model.step(dt, reduced);
      draw();
      if (moving && !reduced) raf = window.requestAnimationFrame(tick);
      else previous = undefined;
    };
    const wake = () => {
      if (disposed || !visible || !intersecting || raf !== undefined) return;
      raf = window.requestAnimationFrame(tick);
    };
    wakeRef.current = wake;
    const resize = () => { size = instrumentCanvasSize(canvas.clientWidth, canvas.clientHeight, window.devicePixelRatio); faceKey = ""; wake(); };
    const visibility = () => { visible = !document.hidden; if (visible) wake(); else stop(); };
    const reduce = () => { reduced = motion.matches; stop(); wake(); };
    const observer = new ResizeObserver(resize);
    const intersection = new IntersectionObserver(entries => { intersecting = entries[0]?.isIntersecting ?? false; if (intersecting) wake(); else stop(); });
    observer.observe(canvas); intersection.observe(canvas);
    document.addEventListener("visibilitychange", visibility); motion.addEventListener("change", reduce);
    wake();
    return () => {
      disposed = true; wakeRef.current = () => {}; stop(); face = null;
      observer.disconnect(); intersection.disconnect();
      document.removeEventListener("visibilitychange", visibility); motion.removeEventListener("change", reduce);
    };
  }, [model]);

  useEffect(() => {
    viewRef.current = { mode, labels };
    model.setTarget(levels, active);
    wakeRef.current();
  }, [model, levels, active, mode, labels.left, labels.right, labels.relative]);

  return <canvas ref={canvasRef} className={`np-instrument-canvas block w-full ${fullscreen ? "h-full min-h-0" : compact ? "h-24 sm:h-28" : "h-52 sm:h-60"}`}
    role="img" aria-label={label} data-instrument={mode} />;
};
