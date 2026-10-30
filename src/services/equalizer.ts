import { invoke } from "@tauri-apps/api/core";
import type { EqCatalog, EqEdit, EqPreset, DspSnapshot } from "../types/equalizer";
export const equalizerService = {
  pickImport: () => invoke<{document:string;preset:EqPreset}|null>("pick_equalizer_preset"),
  exportPreset: (preset:EqPreset) => invoke<boolean>("export_equalizer_preset",{preset}),
  compare: (action:"reference"|"equalized"|"finish",token:string|null,expected:EqPreset|null=null) => invoke<DspSnapshot>("compare_equalizer",{action,token,expected}),
  get: () => invoke<EqCatalog>("get_equalizer"),
  edit: (expectedRevision: string, edit: EqEdit) => invoke<EqCatalog>("edit_equalizer", { expectedRevision, edit }),
  apply: (enabled: boolean, preset: EqPreset | null, bypassExpected: EqPreset | null = null) => invoke<DspSnapshot>("apply_equalizer", { enabled, preset, bypassExpected }),
};
