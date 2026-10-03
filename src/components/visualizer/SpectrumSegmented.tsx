import React from "react";

import { SpectrumCanvas } from "./SpectrumCanvas";

interface SpectrumSegmentedProps {
  spectrum: number[];
  active: boolean;
  label: string;
  fullscreen?: boolean;
}

export const SpectrumSegmented: React.FC<SpectrumSegmentedProps> = (props) => (
  <SpectrumCanvas {...props} style="segmented" />
);
