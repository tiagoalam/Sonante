import { invoke, Channel } from "@tauri-apps/api/core";
import type { LibraryRecoveryObservation, LibraryRecoveryResult } from "../types/libraryRecovery";
import {
  RECOVERY_PROGRESS, DIAGNOSTIC_SCHEMA_VERSION, AVAILABILITIES, FAILURE_CLASSES, FAILURE_BOUNDARIES,
  FAILURE_KNOWLEDGE, POSSIBLE_EFFECTS, INHERITED_OBSERVED, FRESHNESS, OUTCOMES,
  UNAVAILABLE_REASONS, ASSESSMENT_STATUSES, ASSESSMENT_REASONS, ASSESSMENT_ROLES,
  ASSESSMENT_SCOPE,
} from "../types/libraryRecovery";
import type { AttemptDiagnostic, SafeOfferAssessment, RecoveryProgress } from "../types/libraryRecovery";

export const libraryRecoveryService = {
  analyze: (requestId: string) => invoke<LibraryRecoveryObservation>("analyze_library_recovery", { requestId }),
  cancel: (requestId: string) => invoke<void>("cancel_library_recovery_analysis", { requestId }),
  execute: (analysisId: number, planId: string, onProgress?: (stage: RecoveryProgress) => void) => {
    const progress = onProgress ? new Channel<unknown>() : undefined;
    if (progress && onProgress) progress.onmessage = (value) => {
      const stage = RECOVERY_PROGRESS.find((stage) => stage === value);
      if (stage) onProgress(stage);
    };
    return invoke<LibraryRecoveryResult>("execute_library_recovery", { analysisId, planId, ...(progress ? { progress } : {}) });
  },
  reopen: (planId: string) => invoke<void>("reopen_library_after_recovery", { planId }),
  exportReport: (analysisId: number) => invoke<boolean>("export_library_recovery_report", { analysisId }),
  diagnostic: queryAttemptDiagnostic,
};

// No automatic query, timer, retry, logging or legacy error-string interpretation.
function unknownDiagnostic(): AttemptDiagnostic {
  return { kind: "unknown" };
}
const U64_MAX_DECIMAL = "18446744073709551615";

export function canonicalAnalysisKey(value: unknown): value is string {
  if (typeof value !== "string" || value.length < 1 || value.length > 20) return false;
  if (value.length > 1 && value[0] === "0") return false;
  for (const character of value) {
    if (character < "0" || character > "9") return false;
  }
  return value.length < 20 || value <= U64_MAX_DECIMAL;
}

export function diagnosticKeyForAnalysisId(value: unknown): string | null {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) return null;
  // Only exact safe integers. Lost precision in a legacy numeric ID is not recoverable.
  return String(value);
}

function dataRecord(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return false;
  return Reflect.ownKeys(value).every(key => {
    if (typeof key !== "string") return false;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    return descriptor !== undefined && Object.prototype.hasOwnProperty.call(descriptor, "value");
  });
}

function exactKeys(value: Record<string, unknown>, expected: readonly string[]): boolean {
  return Reflect.ownKeys(value).length === expected.length &&
    expected.every(key => Object.prototype.hasOwnProperty.call(value, key));
}

function member<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && allowed.some(label => label === value);
}

function labels<T extends string>(value: unknown, allowed: readonly T[]): readonly T[] | undefined {
  if (!Array.isArray(value) || Object.getPrototypeOf(value) !== Array.prototype) return undefined;
  const lengthDescriptor = Object.getOwnPropertyDescriptor(value, "length");
  if (!lengthDescriptor || !Object.prototype.hasOwnProperty.call(lengthDescriptor, "value")) return undefined;
  const length: unknown = lengthDescriptor.value;
  if (typeof length !== "number" || !Number.isSafeInteger(length) || length < 0 || length > allowed.length) return undefined;
  const keys = Reflect.ownKeys(value);
  if (keys.length !== length + 1 || !keys.includes("length")) return undefined;
  const found = new Set<T>();
  for (let index = 0; index < length; index++) {
    const descriptor = Object.getOwnPropertyDescriptor(value, String(index));
    if (!descriptor || !Object.prototype.hasOwnProperty.call(descriptor, "value")) return undefined;
    const item: unknown = descriptor.value;
    if (!member(item, allowed) || found.has(item)) return undefined;
    found.add(item);
  }
  return Object.freeze(allowed.filter(label => found.has(label)));
}

export function decodeAttemptDiagnostic(value: unknown, expectedKey: string): AttemptDiagnostic {
  try {
    if (!canonicalAnalysisKey(expectedKey) || !dataRecord(value) ||
        !exactKeys(value, ["schema_version", "analysis_key", "availability", "unavailable_reason", "freshness", "outcome", "failure"]) ||
        value.schema_version !== DIAGNOSTIC_SCHEMA_VERSION || value.analysis_key !== expectedKey ||
        !member(value.availability, AVAILABILITIES)) return unknownDiagnostic();
    if (value.availability === "unavailable") {
      if (!member(value.unavailable_reason, UNAVAILABLE_REASONS) || value.freshness !== null ||
          value.outcome !== null || value.failure !== null) return unknownDiagnostic();
      // A valid-key request cannot correlate the backend invalid_lookup/null-key envelope.
      if (value.unavailable_reason === "invalid_lookup") return unknownDiagnostic();
      return { kind: "unavailable", association: expectedKey, reason: value.unavailable_reason };
    }
    if (value.unavailable_reason !== null || !member(value.freshness, FRESHNESS) ||
        !member(value.outcome, OUTCOMES)) return unknownDiagnostic();
    if (value.outcome !== "failed") {
      if (value.failure !== null) return unknownDiagnostic();
      return { kind: "available", association: expectedKey, freshness: value.freshness,
        outcome: value.outcome, failure: null };
    }
    const failure = value.failure;
    if (!dataRecord(failure) || !exactKeys(failure, ["class", "boundary", "knowledge", "possible_effects", "inherited_observed"]) ||
        !member(failure.class, FAILURE_CLASSES) || !member(failure.boundary, FAILURE_BOUNDARIES) ||
        !member(failure.knowledge, FAILURE_KNOWLEDGE)) return unknownDiagnostic();
    const possible = labels(failure.possible_effects, POSSIBLE_EFFECTS);
    const inherited = labels(failure.inherited_observed, INHERITED_OBSERVED);
    if (!possible || !inherited) return unknownDiagnostic();
    if (failure.knowledge === "unknown_boundary" && (failure.class !== "legacy_unknown" || failure.boundary !== "unknown" ||
        possible.length !== POSSIBLE_EFFECTS.length || inherited.length !== 0)) return unknownDiagnostic();
    // Rebuild only fixed safe fields. Missing MAY/fact is not a proof of absence.
    return { kind: "available", association: expectedKey, freshness: value.freshness, outcome: "failed",
      failure: { class: failure.class, boundary: failure.boundary, knowledge: failure.knowledge,
        possibleEffects: possible, inheritedObserved: inherited } };
  } catch {
    return unknownDiagnostic();
  }
}

export async function queryAttemptDiagnostic(analysisId: number): Promise<AttemptDiagnostic> {
  const key = diagnosticKeyForAnalysisId(analysisId);
  if (key === null) return unknownDiagnostic();
  try {
    const raw: unknown = await invoke<unknown>("get_library_recovery_attempt_diagnostic", { analysisKey: key });
    return decodeAttemptDiagnostic(raw, key);
  } catch {
    return unknownDiagnostic();
  }
}

// Extract only the finite optional assessment, not the whole legacy Observation.
// This explains an observation; it is never an execution/reopen capability.
export function extractFiniteOfferAssessment(analyzeResponse: unknown): SafeOfferAssessment | undefined {
  try {
    if (!dataRecord(analyzeResponse) || !Object.prototype.hasOwnProperty.call(analyzeResponse, "assessment")) return undefined;
    const assessment = analyzeResponse.assessment;
    if (!dataRecord(assessment) || assessment.scope !== ASSESSMENT_SCOPE ||
        !member(assessment.status, ASSESSMENT_STATUSES)) return undefined;
    const hasReason = Object.prototype.hasOwnProperty.call(assessment, "reason");
    const hasRole = Object.prototype.hasOwnProperty.call(assessment, "target_role");
    const expected = ["status", "scope"];
    if (hasReason) expected.push("reason");
    if (hasRole) expected.push("target_role");
    if (!exactKeys(assessment, expected)) return undefined;
    const reason = hasReason ? assessment.reason : undefined;
    const role = hasRole ? assessment.target_role : undefined;
    if (hasReason && !member(reason, ASSESSMENT_REASONS)) return undefined;
    if (hasRole && !member(role, ASSESSMENT_ROLES)) return undefined;
    if (assessment.status === "eligible_at_observation" && hasReason) return undefined;
    // A present undefined/null option is not Rust's skip_serializing_if omission.
    if (reason !== undefined && !member(reason, ASSESSMENT_REASONS)) return undefined;
    if (role !== undefined && !member(role, ASSESSMENT_ROLES)) return undefined;
    return { status: assessment.status, reason: reason === undefined ? null : reason,
      targetRole: role === undefined ? null : role };
  } catch {
    return undefined;
  }
}
