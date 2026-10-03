import React, { useEffect, useRef } from "react";

const SPECTRUM_BANDS = 48;

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

interface SpectrumBarsProps {
  spectrum: number[];
  active: boolean;
  label: string;
  large?: boolean;
  fullscreen?: boolean;
}

export const SpectrumBars: React.FC<SpectrumBarsProps> = ({
  spectrum,
  active,
  label,
  large = false,
  fullscreen = false,
}) => {
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

  const heightClass = large
    ? fullscreen
      ? "h-56 lg:h-64"
      : "h-36 sm:h-44 lg:h-48"
    : fullscreen
      ? "h-28"
      : "h-20 sm:h-24";

  return (
    <canvas
      ref={canvasRef}
      className={`block w-full ${heightClass}`}
      role="img"
      aria-label={label}
    />
  );
};
