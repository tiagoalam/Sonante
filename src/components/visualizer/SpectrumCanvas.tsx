import React, { useEffect, useRef } from "react";

const SPECTRUM_BANDS = 48;
const FREQUENCY_MIN = 40;
const FREQUENCY_MAX = 20_000;

export type SpectrumStyle = "neon" | "segmented";

interface SpectrumCanvasProps {
  spectrum: number[];
  active: boolean;
  label: string;
  style: SpectrumStyle;
  compact?: boolean;
  fullscreen?: boolean;
}

const normalized = (value: number) => Math.max(0, Math.min(1, value));

const frequencyPosition = (frequency: number) =>
  Math.log(frequency / FREQUENCY_MIN) / Math.log(FREQUENCY_MAX / FREQUENCY_MIN);

const prepareCanvas = (canvas: HTMLCanvasElement) => {
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
  if (!context) return null;
  context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
  context.clearRect(0, 0, width, height);
  return { context, width, height };
};

const drawGrid = (
  context: CanvasRenderingContext2D,
  width: number,
  bottom: number,
  color: string,
) => {
  context.save();
  context.strokeStyle = color;
  context.lineWidth = 1;
  for (let row = 0; row <= 4; row += 1) {
    const y = 8 + ((bottom - 8) * row) / 4;
    context.beginPath();
    context.moveTo(0, Math.round(y) + 0.5);
    context.lineTo(width, Math.round(y) + 0.5);
    context.stroke();
  }
  for (let column = 0; column <= 12; column += 1) {
    const x = (width * column) / 12;
    context.beginPath();
    context.moveTo(Math.round(x) + 0.5, 8);
    context.lineTo(Math.round(x) + 0.5, bottom);
    context.stroke();
  }
  context.restore();
};

const drawFrequencyScale = (
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  color: string,
) => {
  const labels: Array<[number, string]> = [
    [40, "40"],
    [100, "100"],
    [1_000, "1K"],
    [10_000, "10K"],
    [20_000, "20K"],
  ];
  context.save();
  context.fillStyle = color;
  context.font = "9px ui-monospace, SFMono-Regular, Menlo, monospace";
  context.textBaseline = "bottom";
  labels.forEach(([frequency, text], index) => {
    const x = frequencyPosition(frequency) * width;
    context.textAlign = index === 0 ? "left" : index === labels.length - 1 ? "right" : "center";
    context.fillText(text, x, height);
  });
  context.restore();
};

const drawNeon = (
  canvas: HTMLCanvasElement,
  values: number[],
  peaks: number[],
  compact: boolean,
) => {
  const prepared = prepareCanvas(canvas);
  if (!prepared) return;
  const { context, width, height } = prepared;
  const scaleHeight = compact ? 0 : 18;
  const bottom = height - scaleHeight;

  const backdrop = context.createLinearGradient(0, 0, 0, bottom);
  backdrop.addColorStop(0, "rgba(10, 27, 31, 0.56)");
  backdrop.addColorStop(1, "rgba(2, 7, 9, 0.08)");
  context.fillStyle = backdrop;
  context.fillRect(0, 0, width, bottom);
  drawGrid(context, width, bottom, "rgba(86, 205, 214, 0.075)");

  const gap = Math.max(1, Math.min(4, width / 210));
  const barWidth = Math.max(1, (width - gap * (SPECTRUM_BANDS - 1)) / SPECTRUM_BANDS);
  const gradient = context.createLinearGradient(0, bottom, 0, 0);
  gradient.addColorStop(0, "rgba(30, 120, 129, 0.72)");
  gradient.addColorStop(0.62, "rgba(55, 221, 220, 0.9)");
  gradient.addColorStop(1, "rgba(190, 255, 238, 0.98)");
  context.fillStyle = gradient;
  context.shadowColor = "rgba(57, 224, 220, 0.38)";
  context.shadowBlur = compact ? 4 : 8;

  for (let index = 0; index < SPECTRUM_BANDS; index += 1) {
    const value = normalized(values[index] ?? 0);
    const barHeight = value * Math.max(0, bottom - 10);
    const x = index * (barWidth + gap);
    context.fillRect(x, bottom - barHeight, barWidth, barHeight);
  }

  context.shadowBlur = 0;
  context.fillStyle = "rgba(218, 255, 247, 0.78)";
  for (let index = 0; index < SPECTRUM_BANDS; index += 1) {
    const peak = normalized(peaks[index] ?? 0);
    if (peak <= 0) continue;
    const x = index * (barWidth + gap);
    const y = bottom - peak * Math.max(0, bottom - 10);
    context.fillRect(x, Math.max(1, y - 1), barWidth, 1);
  }

  if (!compact) drawFrequencyScale(context, width, height, "rgba(163, 219, 219, 0.42)");
};

const drawSegmented = (
  canvas: HTMLCanvasElement,
  values: number[],
  peaks: number[],
  compact: boolean,
) => {
  const prepared = prepareCanvas(canvas);
  if (!prepared) return;
  const { context, width, height } = prepared;
  const scaleHeight = compact ? 0 : 18;
  const bottom = height - scaleHeight;
  const segmentCount = compact ? 12 : 20;
  const verticalGap = compact ? 2 : 3;
  const segmentHeight = Math.max(2, (bottom - 12 - verticalGap * segmentCount) / segmentCount);
  const gap = Math.max(1, Math.min(3, width / 230));
  const barWidth = Math.max(1, (width - gap * (SPECTRUM_BANDS - 1)) / SPECTRUM_BANDS);

  context.fillStyle = "rgba(5, 12, 15, 0.72)";
  context.fillRect(0, 0, width, bottom);
  drawGrid(context, width, bottom, "rgba(104, 170, 181, 0.06)");

  for (let index = 0; index < SPECTRUM_BANDS; index += 1) {
    const value = normalized(values[index] ?? 0);
    const litSegments = Math.round(value * segmentCount);
    const x = index * (barWidth + gap);
    for (let segment = 0; segment < segmentCount; segment += 1) {
      const y = bottom - 5 - (segment + 1) * (segmentHeight + verticalGap);
      if (segment < litSegments) {
        const position = segment / Math.max(1, segmentCount - 1);
        context.fillStyle =
          position > 0.82
            ? "rgba(244, 184, 72, 0.94)"
            : position > 0.58
              ? "rgba(92, 218, 196, 0.92)"
              : "rgba(42, 151, 161, 0.86)";
      } else {
        context.fillStyle = "rgba(93, 147, 154, 0.08)";
      }
      context.fillRect(x, y, barWidth, segmentHeight);
    }

    const peakSegment = Math.min(segmentCount - 1, Math.floor(normalized(peaks[index] ?? 0) * segmentCount));
    if (peaks[index] > 0) {
      const y = bottom - 5 - (peakSegment + 1) * (segmentHeight + verticalGap);
      context.fillStyle = "rgba(225, 244, 236, 0.68)";
      context.fillRect(x, y, barWidth, Math.max(1, segmentHeight * 0.35));
    }
  }

  if (!compact) drawFrequencyScale(context, width, height, "rgba(154, 198, 202, 0.4)");
};

export const SpectrumCanvas: React.FC<SpectrumCanvasProps> = ({
  spectrum,
  active,
  label,
  style,
  compact = false,
  fullscreen = false,
}) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const targetRef = useRef<number[]>(Array(SPECTRUM_BANDS).fill(0));
  const displayedRef = useRef<number[]>(Array(SPECTRUM_BANDS).fill(0));
  const peakRef = useRef<number[]>(Array(SPECTRUM_BANDS).fill(0));
  const animationRef = useRef<number | undefined>(undefined);
  const previousTimeRef = useRef<number | undefined>(undefined);
  const drawRef = useRef(style === "neon" ? drawNeon : drawSegmented);
  drawRef.current = style === "neon" ? drawNeon : drawSegmented;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() =>
      drawRef.current(canvas, displayedRef.current, peakRef.current, compact),
    );
    observer.observe(canvas);
    drawRef.current(canvas, displayedRef.current, peakRef.current, compact);
    return () => {
      observer.disconnect();
      if (animationRef.current !== undefined) window.cancelAnimationFrame(animationRef.current);
      animationRef.current = undefined;
      previousTimeRef.current = undefined;
    };
  }, [compact]);

  useEffect(() => {
    const receivingSpectrum = active && spectrum.length > 0;
    targetRef.current = Array.from(
      { length: SPECTRUM_BANDS },
      (_, index) => (receivingSpectrum ? normalized(spectrum[index] ?? 0) : 0),
    );
    if (animationRef.current !== undefined) return;

    const animate = (time: number) => {
      const canvas = canvasRef.current;
      if (!canvas) {
        animationRef.current = undefined;
        return;
      }
      const elapsed = Math.min(50, time - (previousTimeRef.current ?? time));
      previousTimeRef.current = time;
      let moving = false;

      for (let index = 0; index < SPECTRUM_BANDS; index += 1) {
        const target = targetRef.current[index];
        const current = displayedRef.current[index];
        const factor = target > current ? 0.48 : 0.14;
        const next = current + (target - current) * factor;
        displayedRef.current[index] = Math.abs(target - next) < 0.001 ? target : next;

        if (target > peakRef.current[index]) {
          peakRef.current[index] = target;
        } else {
          const decayPerMillisecond = receivingSpectrum ? 0.00022 : 0.0012;
          peakRef.current[index] = Math.max(
            target,
            peakRef.current[index] - elapsed * decayPerMillisecond,
          );
        }
        if (
          Math.abs(displayedRef.current[index] - target) > 0.001 ||
          peakRef.current[index] > target + 0.001
        ) {
          moving = true;
        }
      }

      drawRef.current(canvas, displayedRef.current, peakRef.current, compact);
      if (moving) {
        animationRef.current = window.requestAnimationFrame(animate);
      } else {
        animationRef.current = undefined;
        previousTimeRef.current = undefined;
      }
    };
    animationRef.current = window.requestAnimationFrame(animate);
  }, [active, compact, spectrum, style]);

  const heightClass = compact
    ? fullscreen
      ? "h-[clamp(96px,14vh,144px)]"
      : "h-24 sm:h-28"
    : fullscreen
      ? "h-full min-h-0"
      : "h-52 sm:h-60";

  return (
    <canvas
      ref={canvasRef}
      className={`block w-full ${heightClass}`}
      role="img"
      aria-label={label}
    />
  );
};
