import React from "react";
import type { AudioLevelFrame } from "../../types/audio";
import { InstrumentCanvas } from "./InstrumentCanvas";
export type AnalogVuVariant = "amber" | "blue" | "radial";
interface AnalogVuProps {
  levels: AudioLevelFrame; active: boolean; variant: AnalogVuVariant;
  leftLabel: string; rightLabel: string; relativeLabel: string; fullscreen?: boolean;
}
export const AnalogVu: React.FC<AnalogVuProps> = ({ levels, active, variant, leftLabel, rightLabel, relativeLabel, fullscreen }) =>
  <InstrumentCanvas levels={levels} active={active} mode={variant === "radial" ? "vu-radial" : variant === "blue" ? "vu-blue" : "vu-amber"}
    labels={{ left: leftLabel, right: rightLabel, relative: relativeLabel }} label={`${leftLabel} / ${rightLabel} — ${relativeLabel} · RMS dBFS`} fullscreen={fullscreen} />;
