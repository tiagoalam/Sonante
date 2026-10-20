export type LibraryRecoveryReason =
  | "intent_valid" | "intent_invalid" | "orphan_staging" | "config_missing"
  | "config_invalid" | "config_divergent" | "mirror_missing" | "mirror_invalid"
  | "generation_divergent" | "database_missing" | "database_unproven"
  | "ledger_missing" | "ledger_divergent" | "unsafe_entry" | "filesystem_changed"
  | "ownership_unknown" | "no_pending_marker" | "recovery_set_unproven";

export interface LibraryRecoveryObservation {
  analysis_id: number;
  reasons: LibraryRecoveryReason[];
  intent_version: number | null;
  queue_backup_version: number | null;
  config_matches: "previous" | "next" | "both" | "unbound" | "unknown";
  recovery_available: boolean;
  plan?: LibraryRecoveryPlan;
}

export interface LibraryRecoveryPlan {
  id: string;
  choice: "restore_previous" | "finish_confirmed";
  previous_tracks: number;
  backup_bytes: number;
  preserves_current_references: boolean;
  resumes_interrupted: boolean;
}
export interface LibraryRecoveryResult {
  completed: boolean;
  backup_retained: boolean;
  requires_reopen: boolean;
}
