import type { LocalAlbumTrackInspection, LocalTrackTagsError } from "./localTags";

// The general Rust DoctorError is a serialized string, unlike per-file errors.
export type DoctorError = "state_unavailable" | "catalog_read_failed" | "album_not_found" | "task_failed";
export type DoctorUiError = DoctorError | "unknown" | "report_mismatch" | "invalid_report" | "read_in_progress";
export type DoctorFindingCode =
  | "album_artist_absent" | "album_artist_empty" | "date_uninterpretable"
  | "year_divergence" | "number_indeterminate" | "track_exceeds_total"
  | "disc_exceeds_total" | "possible_number_conflict" | "cue_checks_not_performed";
export type DoctorFindingLevel = "advisory" | "inconsistency" | "possible_conflict";
export type DoctorField = "album_artist" | "date" | "track_number" | "track_total" | "disc_number" | "disc_total" | "cue";
export type DoctorUnperformedCheck = "missing_tracks" | "content_duplicates" | "canonicalization_discarded_entries" | "cue_metadata" | "cue_numbering";

export interface DoctorEvidence {
  uri: string;
  field: DoctorField;
  value: string | null;
}
export interface DoctorFinding {
  code: DoctorFindingCode;
  level: DoctorFindingLevel;
  evidence: DoctorEvidence[];
}
export interface DoctorCoverage {
  canonical_track_count: number;
  physical_track_count: number;
  physical_tracks_read: number;
  failed_physical_track_count: number;
  cue_virtual_count: number;
  physical_inspection_complete: boolean;
  partial: boolean;
  checks_not_performed: DoctorUnperformedCheck[];
}
export interface LibraryDoctorReport {
  album_id: string;
  tracks: LocalAlbumTrackInspection[];
  coverage: DoctorCoverage;
  findings: DoctorFinding[];
  failures: { uri: string; error: LocalTrackTagsError }[];
}
