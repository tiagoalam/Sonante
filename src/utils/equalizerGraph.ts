import type { EqPreset } from "../types/equalizer";
import { clampGain, responseDb, xFrequency } from "./equalizer.ts";

export const EQ_GRAPH_SAMPLE_COUNT = 481;
export const EQ_GRAPH_MIN_RANGE_DB = 4;
export const EQ_GRAPH_MAX_RANGE_DB = 600;

const rangeSteps = [4, 6, 9, 12, 15, 18, 24, 30, 36, 48, 60, 75, 90, 120, 150, 180, 240, 300, 360, 480, 600];
const frequencyTicks = [20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 20_000];
const compactFrequencyLabels = [20, 100, 500, 2_000, 5_000, 20_000];

export function equalizerGraphTicks(range: number): number[] {
  const limit = Number.isFinite(range)
    ? Math.max(EQ_GRAPH_MIN_RANGE_DB, Math.min(EQ_GRAPH_MAX_RANGE_DB, range))
    : EQ_GRAPH_MIN_RANGE_DB;
  return Array.from({ length: 7 }, (_, index) => Number((-limit + index * limit / 3).toFixed(1)));
}

export function equalizerGraphFrequencyLabels(plotWidth: number): number[] {
  return Number.isFinite(plotWidth) && plotWidth < 460 ? [...compactFrequencyLabels] : [...frequencyTicks];
}

export function equalizerGraphFrequencies(count = EQ_GRAPH_SAMPLE_COUNT): number[] {
  const safeCount = Number.isFinite(count) ? Math.max(2, Math.min(2049, Math.floor(count))) : EQ_GRAPH_SAMPLE_COUNT;
  return Array.from({ length: safeCount }, (_, index) => xFrequency(index / (safeCount - 1)));
}

export function sampleEqualizerGraph(preset: EqPreset, effectivePreampDb: number | null, includeProtected: boolean) {
  const frequencies = equalizerGraphFrequencies();
  return {
    frequencies,
    individual: preset.bands.map(band => band.enabled
      ? frequencies.map(frequency => responseDb({ ...preset, preamp_db: 0, bands: [band] }, frequency))
      : null),
    requested: frequencies.map(frequency => responseDb({ ...preset, preamp_db: 0 }, frequency)),
    combined: includeProtected && effectivePreampDb !== null && Number.isFinite(effectivePreampDb)
      ? frequencies.map(frequency => responseDb({ ...preset, preamp_db: effectivePreampDb }, frequency))
      : null,
  };
}

export function equalizerGraphRange(curves: readonly (readonly number[] | null)[]): number {
  let extent = 0;
  for (const curve of curves) {
    if (!curve) continue;
    for (const value of curve) if (Number.isFinite(value)) extent = Math.max(extent, Math.abs(value));
  }
  const target = Math.max(EQ_GRAPH_MIN_RANGE_DB, extent * 1.12);
  return rangeSteps.find(step => step >= target) ?? EQ_GRAPH_MAX_RANGE_DB;
}

function safeRange(range: number): number {
  return Number.isFinite(range)
    ? Math.max(EQ_GRAPH_MIN_RANGE_DB, Math.min(EQ_GRAPH_MAX_RANGE_DB, range))
    : EQ_GRAPH_MIN_RANGE_DB;
}

export function equalizerGraphY(db: number, top: number, height: number, range: number): number {
  const value = Number.isFinite(db) ? db : 0;
  return top + height / 2 - value * height / (2 * safeRange(range));
}

export function equalizerGraphGain(y: number, top: number, height: number, range: number): number {
  if (!Number.isFinite(y) || !Number.isFinite(top) || !Number.isFinite(height) || height <= 0) return 0;
  return clampGain((0.5 - (y - top) / height) * 2 * safeRange(range));
}

export function equalizerGraphPath(values: readonly number[], left: number, width: number, top: number, height: number, range: number): string {
  if (!values.length || !Number.isFinite(left) || !Number.isFinite(width) || width < 0) return "";
  const path: string[] = [];
  let drawing = false;
  values.forEach((value, index) => {
    if (!Number.isFinite(value)) { drawing = false; return; }
    const x = left + (values.length === 1 ? 0 : index * width / (values.length - 1));
    const y = equalizerGraphY(value, top, height, range);
    path.push(`${drawing ? "L" : "M"}${x},${y}`);
    drawing = true;
  });
  return path.join(" ");
}
