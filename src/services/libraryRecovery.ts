import { invoke } from "@tauri-apps/api/core";
import type { LibraryRecoveryObservation, LibraryRecoveryResult } from "../types/libraryRecovery";

export const libraryRecoveryService = {
  analyze: (requestId: string) => invoke<LibraryRecoveryObservation>("analyze_library_recovery", { requestId }),
  cancel: (requestId: string) => invoke<void>("cancel_library_recovery_analysis", { requestId }),
  execute: (analysisId: number, planId: string) => invoke<LibraryRecoveryResult>("execute_library_recovery", { analysisId, planId }),
  reopen: (planId: string) => invoke<void>("reopen_library_after_recovery", { planId }),
  exportReport: (analysisId: number) => invoke<boolean>("export_library_recovery_report", { analysisId }),
};
