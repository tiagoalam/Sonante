import type { DoctorFindingCode } from "../types/libraryDoctor";
import type { DoctorSessionState } from "./libraryDoctor";

export type DoctorReviewCode = "album_artist_absent" | "album_artist_empty" | "year_divergence";
export interface DoctorReviewContext {
  albumId: string;
  code: DoctorReviewCode;
  field: "album_artist" | "date";
}

export function doctorReviewField(code: DoctorFindingCode): DoctorReviewContext["field"] | null {
  if (code === "album_artist_absent" || code === "album_artist_empty") return "album_artist";
  return code === "year_divergence" ? "date" : null;
}

// Only a navigation hint crosses into TAG-2: never tags, revisions or values.
export function doctorReviewRequest(
  state: DoctorSessionState,
  code: DoctorFindingCode,
  current: { albumId: string; catalogVersion: unknown; available: boolean; editorOpen: boolean },
): DoctorReviewContext | null {
  const field = doctorReviewField(code), report = state.report;
  if (!field || !current.available || current.editorOpen || !state.available || state.inFlight
    || state.phase !== "ready" || state.albumId !== current.albumId
    || state.catalogVersion !== current.catalogVersion || report?.album_id !== current.albumId
    || !report.findings.some(finding => finding.code === code)) return null;
  const c = report.coverage;
  if (!c.physical_inspection_complete || c.partial || c.physical_track_count <= 0
    || c.physical_tracks_read !== c.physical_track_count || c.failed_physical_track_count !== 0
    || c.cue_virtual_count !== 0 || report.failures.length !== 0
    || c.canonical_track_count !== c.physical_track_count || report.tracks.length !== c.physical_track_count
    || report.tracks.some(track => track.kind !== "physical" || track.issue || !track.tags?.readable
      || !track.tags.writable || !track.tags.editable)) return null;
  return { albumId: current.albumId, code: code as DoctorReviewCode, field };
}
