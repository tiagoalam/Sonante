import React from "react";

import { SpectrumCanvas } from "./SpectrumCanvas";

interface SpectrumNeonProps {
  spectrum: number[];
  active: boolean;
  label: string;
  compact?: boolean;
  fullscreen?: boolean;
}

export const SpectrumNeon: React.FC<SpectrumNeonProps> = (props) => (
  <SpectrumCanvas {...props} style="neon" />
);
