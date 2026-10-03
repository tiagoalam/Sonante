import React from "react";

import type { AudioLevelFrame } from "../../types/audio";

interface VuHorizontalProps {
  levels: AudioLevelFrame;
  active: boolean;
  large?: boolean;
  fullscreen?: boolean;
}

const normalized = (value: number) => Math.max(0, Math.min(1, value));

export const VuHorizontal: React.FC<VuHorizontalProps> = ({
  levels,
  active,
  large = false,
  fullscreen = false,
}) => (
  <div className={large ? (fullscreen ? "space-y-8" : "space-y-5") : "space-y-1.5"}>
    {(["L", "R"] as const).map((channel) => {
      const rms = active
        ? normalized(channel === "L" ? levels.leftRms : levels.rightRms)
        : 0;
      const peak = active
        ? normalized(channel === "L" ? levels.leftPeak : levels.rightPeak)
        : 0;
      const meterHeight = large ? (fullscreen ? "h-16" : "h-10 sm:h-12") : "h-2";

      return (
        <div key={channel} className={`flex items-center ${large ? "gap-4" : "gap-2"}`}>
          <span
            className={`font-mono font-semibold text-white/55 ${
              large ? "w-6 text-base sm:text-lg" : "w-3 text-[10px]"
            }`}
          >
            {channel}
          </span>
          <div
            className={`relative flex-1 overflow-hidden rounded-full bg-white/8 ${meterHeight}`}
          >
            <div
              className="h-full rounded-full bg-gradient-to-r from-[#9A6B05] via-[#D39209] to-[#E5A00D] transition-[width] duration-100 ease-out"
              style={{ width: `${rms * 100}%` }}
            />
            <span
              className="absolute top-0 h-full w-0.5 -translate-x-px bg-white/90 transition-[left] duration-100 ease-out"
              style={{ left: `${peak * 100}%`, opacity: peak > 0 ? 1 : 0 }}
            />
          </div>
        </div>
      );
    })}
  </div>
);
