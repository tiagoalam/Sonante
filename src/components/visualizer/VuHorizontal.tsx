import React from "react";
import type { AudioLevelFrame } from "../../types/audio";
import { InstrumentCanvas } from "./InstrumentCanvas";
interface VuHorizontalProps { levels: AudioLevelFrame; active: boolean; large?: boolean; fullscreen?: boolean; }
export const VuHorizontal: React.FC<VuHorizontalProps> = ({ levels, active, large, fullscreen }) =>
  <InstrumentCanvas levels={levels} active={active} mode="levels" label="L / R · RMS · dBFS" compact={!large} fullscreen={fullscreen} />;
