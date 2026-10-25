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

// Sanitized diagnostic observations only; none of these fields grants action authority.
export const DIAGNOSTIC_SCHEMA_VERSION = 1 as const;
export const AVAILABILITIES = ["available", "unavailable"] as const;
export const FAILURE_CLASSES = ["legacy_unknown", "cancellation_observed", "preflight_unconfirmed", "plan_context_changed", "offered_context_unconfirmed", "local_journal_context_unconfirmed", "catalog_mismatch_observed"] as const;
export const FAILURE_BOUNDARIES = ["protocol_check", "readonly_preflight", "protection_records", "local_journal_binding", "catalog_check", "generation_namespace", "control_writes", "final_validation", "commit_retention", "unknown"] as const;
export const FAILURE_KNOWLEDGE = ["executor_captured", "unknown_boundary"] as const;
export const POSSIBLE_EFFECTS = ["protection", "live_generation_namespace", "control_temporary", "canonical_control", "retention", "backup_namespace", "validator_or_child_unknown", "opaque_callback"] as const;
export const INHERITED_OBSERVED = ["journal_and_blocked_read", "committed_name_not_completed"] as const;
export const FRESHNESS = ["current", "stale"] as const;
export const OUTCOMES = ["not_attempted", "unconfirmed", "failed", "state_returned_ok"] as const;
export const UNAVAILABLE_REASONS = ["invalid_lookup", "shutdown", "not_captured_or_unavailable"] as const;
export const ASSESSMENT_STATUSES = ["eligible_at_observation", "ineligible", "unconfirmed"] as const;
export const ASSESSMENT_REASONS = ["ownership_unconfirmed", "planning_unconfirmed", "plan_changed", "historical_mismatch", "historical_proof_incomparable", "original_root_unavailable", "original_root_permission_denied", "origin_unconfirmed", "ledger_invalid", "ledger_extension_unconfirmed", "manifest_unconfirmed", "unsupported_format", "unsafe_or_ambiguous", "snapshot_changed", "incomplete_set", "cancelled", "unknown"] as const;
export const ASSESSMENT_ROLES = ["previous", "completion", "confirmed", "journal_resume", "journal_committed"] as const;
export const ASSESSMENT_SCOPE = "readonly_offer_only" as const;

export type DiagnosticAvailability = (typeof AVAILABILITIES)[number];
export type FailureClass = (typeof FAILURE_CLASSES)[number];
export type FailureBoundary = (typeof FAILURE_BOUNDARIES)[number];
export type FailureKnowledge = (typeof FAILURE_KNOWLEDGE)[number];
export type PossibleEffect = (typeof POSSIBLE_EFFECTS)[number];
export type ObservedInherited = (typeof INHERITED_OBSERVED)[number];
export type Freshness = (typeof FRESHNESS)[number];
export type Outcome = (typeof OUTCOMES)[number];
export type UnavailableReason = (typeof UNAVAILABLE_REASONS)[number];
export type AssessmentStatus = (typeof ASSESSMENT_STATUSES)[number];
export type AssessmentReason = (typeof ASSESSMENT_REASONS)[number];
export type AssessmentRole = (typeof ASSESSMENT_ROLES)[number];

export interface SafeDiagnosticFailure {
  readonly class: FailureClass;
  readonly boundary: FailureBoundary;
  readonly knowledge: FailureKnowledge;
  readonly possibleEffects: readonly PossibleEffect[];
  readonly inheritedObserved: readonly ObservedInherited[];
}
export type AttemptDiagnostic =
  | Readonly<{ kind: "unknown" }>
  | Readonly<{ kind: "unavailable"; association: string; reason: UnavailableReason }>
  | Readonly<{ kind: "available"; association: string; freshness: Freshness;
      outcome: "not_attempted" | "unconfirmed" | "state_returned_ok"; failure: null }>
  | Readonly<{ kind: "available"; association: string; freshness: Freshness;
      outcome: "failed"; failure: SafeDiagnosticFailure }>;

export interface SafeOfferAssessment {
  readonly status: AssessmentStatus;
  readonly reason: AssessmentReason | null;
  readonly targetRole: AssessmentRole | null;
}
