import React, { useEffect, useRef } from "react";

import type { AudioLevelFrame } from "../../types/audio";

export type AnalogVuVariant = "amber" | "blue";

interface AnalogVuProps {
  levels: AudioLevelFrame;
  active: boolean;
  variant: AnalogVuVariant;
  leftLabel: string;
  rightLabel: string;
  relativeLabel: string;
  fullscreen?: boolean;
}

interface VuTheme {
  panelTop: string;
  panelBottom: string;
  border: string;
  arc: string;
  tick: string;
  text: string;
  accent: string;
  needle: string;
  hub: string;
  glow: string;
}

const THEMES: Record<AnalogVuVariant, VuTheme> = {
  amber: {
    panelTop: "#6f481c",
    panelBottom: "#211205",
    border: "rgba(240, 179, 81, 0.3)",
    arc: "rgba(255, 198, 99, 0.3)",
    tick: "rgba(255, 216, 145, 0.75)",
    text: "rgba(255, 226, 174, 0.86)",
    accent: "rgba(255, 170, 57, 0.96)",
    needle: "rgba(255, 239, 207, 0.96)",
    hub: "#f0b25b",
    glow: "rgba(255, 151, 43, 0.22)",
  },
  blue: {
    panelTop: "#102b3b",
    panelBottom: "#040c13",
    border: "rgba(89, 205, 246, 0.3)",
    arc: "rgba(91, 208, 247, 0.28)",
    tick: "rgba(159, 231, 255, 0.74)",
    text: "rgba(190, 239, 255, 0.86)",
    accent: "rgba(63, 211, 255, 0.98)",
    needle: "rgba(225, 249, 255, 0.98)",
    hub: "#5bd9ff",
    glow: "rgba(43, 193, 255, 0.22)",
  },
};

const normalized = (value: number) => Math.max(0, Math.min(1, value));
const START_ANGLE = Math.PI * 1.12;
const END_ANGLE = Math.PI * 1.88;
const valueAngle = (value: number) => START_ANGLE + normalized(value) * (END_ANGLE - START_ANGLE);

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

const drawMeter = (
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  rms: number,
  label: string,
  relativeLabel: string,
  theme: VuTheme,
) => {
  const inset = Math.max(6, width * 0.018);
  const radius = Math.max(8, Math.min(width * 0.39, height * 0.6));
  const centerX = x + width / 2;
  const centerY = y + height * 0.79;
  const gradient = context.createLinearGradient(0, y, 0, y + height);
  gradient.addColorStop(0, theme.panelTop);
  gradient.addColorStop(0.62, theme.panelBottom);
  gradient.addColorStop(1, "#030303");

  context.save();
  context.fillStyle = gradient;
  context.strokeStyle = theme.border;
  context.lineWidth = 1;
  context.beginPath();
  context.roundRect(x + inset, y + inset, width - inset * 2, height - inset * 2, 14);
  context.fill();
  context.stroke();

  context.strokeStyle = theme.arc;
  context.lineWidth = 2;
  context.beginPath();
  context.arc(centerX, centerY, radius, START_ANGLE, END_ANGLE);
  context.stroke();

  const tickCount = 20;
  for (let tick = 0; tick <= tickCount; tick += 1) {
    const fraction = tick / tickCount;
    const angle = valueAngle(fraction);
    const major = tick % 5 === 0;
    const inner = radius - (major ? 14 : 7);
    context.strokeStyle = major ? theme.tick : theme.arc;
    context.lineWidth = major ? 1.5 : 1;
    context.beginPath();
    context.moveTo(centerX + Math.cos(angle) * inner, centerY + Math.sin(angle) * inner);
    context.lineTo(centerX + Math.cos(angle) * radius, centerY + Math.sin(angle) * radius);
    context.stroke();
  }

  context.fillStyle = theme.text;
  context.font = `${Math.max(9, Math.min(12, width * 0.026))}px ui-monospace, SFMono-Regular, Menlo, monospace`;
  context.textAlign = "center";
  context.textBaseline = "middle";
  [0, 0.25, 0.5, 0.75, 1].forEach((value) => {
    const angle = valueAngle(value);
    const labelRadius = radius - 27;
    context.fillText(
      value.toFixed(value === 0 || value === 1 ? 0 : 2).replace(/^0/, ""),
      centerX + Math.cos(angle) * labelRadius,
      centerY + Math.sin(angle) * labelRadius,
    );
  });

  const angle = valueAngle(rms);
  context.strokeStyle = theme.accent;
  context.lineWidth = 3;
  context.shadowColor = theme.glow;
  context.shadowBlur = 8;
  context.beginPath();
  context.moveTo(
    centerX + Math.cos(angle) * (radius + 1),
    centerY + Math.sin(angle) * (radius + 1),
  );
  context.lineTo(
    centerX + Math.cos(angle) * (radius - 10),
    centerY + Math.sin(angle) * (radius - 10),
  );
  context.stroke();

  context.strokeStyle = theme.needle;
  context.lineWidth = 2;
  context.shadowColor = theme.glow;
  context.shadowBlur = 10;
  context.beginPath();
  context.moveTo(centerX - Math.cos(angle) * 10, centerY - Math.sin(angle) * 10);
  context.lineTo(
    centerX + Math.cos(angle) * (radius - 15),
    centerY + Math.sin(angle) * (radius - 15),
  );
  context.stroke();
  context.shadowBlur = 0;

  context.fillStyle = theme.hub;
  context.beginPath();
  context.arc(centerX, centerY, 7, 0, Math.PI * 2);
  context.fill();
  context.fillStyle = "rgba(0, 0, 0, 0.72)";
  context.beginPath();
  context.arc(centerX, centerY, 3, 0, Math.PI * 2);
  context.fill();

  context.fillStyle = theme.text;
  context.font = `600 ${Math.max(10, Math.min(14, width * 0.032))}px ui-monospace, SFMono-Regular, Menlo, monospace`;
  context.letterSpacing = "0.14em";
  context.fillText(label, centerX, y + height * 0.15);
  context.globalAlpha = 0.55;
  context.font = `${Math.max(8, Math.min(10, width * 0.022))}px ui-monospace, SFMono-Regular, Menlo, monospace`;
  context.fillText(relativeLabel, centerX, y + height - 18);
  context.restore();
};

export const AnalogVu: React.FC<AnalogVuProps> = ({
  levels,
  active,
  variant,
  leftLabel,
  rightLabel,
  relativeLabel,
  fullscreen = false,
}) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const targetRef = useRef([0, 0]);
  const displayedRef = useRef([0, 0]);
  const animationRef = useRef<number | undefined>(undefined);
  const drawRef = useRef<() => void>(() => undefined);

  drawRef.current = () => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const prepared = prepareCanvas(canvas);
    if (!prepared) return;
    const { context, width, height } = prepared;
    const gap = Math.max(8, width * 0.018);
    const meterWidth = (width - gap) / 2;
    const theme = THEMES[variant];
    drawMeter(
      context,
      0,
      0,
      meterWidth,
      height,
      displayedRef.current[0],
      leftLabel,
      relativeLabel,
      theme,
    );
    drawMeter(
      context,
      meterWidth + gap,
      0,
      meterWidth,
      height,
      displayedRef.current[1],
      rightLabel,
      relativeLabel,
      theme,
    );
  };

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() => drawRef.current());
    observer.observe(canvas);
    drawRef.current();
    return () => {
      observer.disconnect();
      if (animationRef.current !== undefined) window.cancelAnimationFrame(animationRef.current);
      animationRef.current = undefined;
    };
  }, []);

  useEffect(() => {
    targetRef.current = active
      ? [normalized(levels.leftRms), normalized(levels.rightRms)]
      : [0, 0];
    if (animationRef.current !== undefined) return;

    const animate = () => {
      let moving = false;
      for (let channel = 0; channel < 2; channel += 1) {
        const current = displayedRef.current[channel];
        const target = targetRef.current[channel];
        const factor = target > current ? 0.24 : 0.09;
        const next = current + (target - current) * factor;
        displayedRef.current[channel] = Math.abs(target - next) < 0.0008 ? target : next;
        if (Math.abs(displayedRef.current[channel] - target) > 0.0008) moving = true;
      }
      drawRef.current();
      if (moving) {
        animationRef.current = window.requestAnimationFrame(animate);
      } else {
        animationRef.current = undefined;
      }
    };
    animationRef.current = window.requestAnimationFrame(animate);
  }, [active, levels.leftRms, levels.rightRms, variant]);

  return (
    <canvas
      ref={canvasRef}
      className={`block w-full ${fullscreen ? "h-full min-h-0" : "h-52 sm:h-60"}`}
      role="img"
      aria-label={`${leftLabel} / ${rightLabel} — ${relativeLabel}`}
    />
  );
};
