import React from "react";
import { InstrumentCanvas } from "./InstrumentCanvas";
export type SpectrumStyle = "neon" | "segmented";
interface SpectrumCanvasProps { spectrum: number[]; active: boolean; label: string; style: SpectrumStyle; compact?: boolean; fullscreen?: boolean; }
export const SpectrumCanvas: React.FC<SpectrumCanvasProps> = ({ spectrum, active, label, style, compact, fullscreen }) =>
  <InstrumentCanvas levels={{ leftRms: 0, rightRms: 0, leftPeak: 0, rightPeak: 0, spectrum }} active={active}
    mode={style === "segmented" ? "spectrum-segmented" : "spectrum-neon"} label={label} compact={compact} fullscreen={fullscreen} />;
