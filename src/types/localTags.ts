export interface LocalTrackTags {
  title: string | null;
  artist: string | null;
  album: string | null;
  album_artist: string | null;
  genre: string | null;
  date: string | null;
  track_number: number | null;
  track_total: number | null;
  disc_number: number | null;
  disc_total: number | null;
  format: string;
  readable: boolean;
  writable: boolean;
  editable: boolean;
  unsupported_reason: string | null;
  revision: string;
}

export type TagFieldUpdate<T> =
  | { action: "unchanged" }
  | { action: "set"; value: T }
  | { action: "remove" };

export interface LocalTrackTagUpdate {
  title: TagFieldUpdate<string>;
  artist: TagFieldUpdate<string>;
  album: TagFieldUpdate<string>;
  album_artist: TagFieldUpdate<string>;
  genre: TagFieldUpdate<string>;
  date: TagFieldUpdate<string>;
  track_number: TagFieldUpdate<number>;
  track_total: TagFieldUpdate<number>;
  disc_number: TagFieldUpdate<number>;
  disc_total: TagFieldUpdate<number>;
}

export type LocalTagMpdRefreshErrorCode =
  | "invalid_uri"
  | "mpd_unavailable"
  | "update_command_failed"
  | "invalid_response"
  | "refresh_task_failed";

export interface LocalTrackTagUpdateResult {
  committed: boolean;
  tags: LocalTrackTags;
  mpd_refresh: {
    requested: boolean;
    success: boolean;
    job_id: number | null;
    error_code: LocalTagMpdRefreshErrorCode | null;
  };
}

export type LocalTrackTagsErrorCode =
  | "invalid_local_path"
  | "outside_configured_roots"
  | "file_not_found"
  | "cue_virtual"
  | "unsupported_format"
  | "read_failed"
  | "invalid_tag_update"
  | "revision_conflict"
  | "permission_denied"
  | "transaction_failed"
  | "metadata_preservation_failed"
  | "durability_failed";

export interface LocalTrackTagsError {
  code: LocalTrackTagsErrorCode;
  message?: string;
  format?: string | null;
  editable?: boolean;
  unsupported_reason?: string | null;
  committed?: boolean;
}

export type LocalAlbumTrackKind = "physical" | "cue_virtual";

export type LocalAlbumFieldState =
  | "common"
  | "absent"
  | "mixed"
  | "indeterminate"
  | "not_applicable";

export interface LocalAlbumFieldSummary {
  state: LocalAlbumFieldState;
  value: string | null;
}

export interface LocalAlbumTagFields {
  album: LocalAlbumFieldSummary;
  album_artist: LocalAlbumFieldSummary;
  date: LocalAlbumFieldSummary;
  genre: LocalAlbumFieldSummary;
}

export interface LocalAlbumTrackInspection {
  uri: string;
  kind: LocalAlbumTrackKind;
  tags: LocalTrackTags | null;
  issue: LocalTrackTagsError | null;
}

export interface LocalAlbumTagInspection {
  album_id: string;
  tracks: LocalAlbumTrackInspection[];
  fields: LocalAlbumTagFields;
  physical_track_count: number;
  cue_virtual_count: number;
  inspection_complete: boolean;
}

export interface LocalAlbumTagBatchExpectedTrack {
  uri: string;
  kind: LocalAlbumTrackKind;
  revision: string | null;
}

export interface LocalAlbumTagBatchUpdate {
  album: TagFieldUpdate<string>;
  album_artist: TagFieldUpdate<string>;
  date: TagFieldUpdate<string>;
  genre: TagFieldUpdate<string>;
}

export interface LocalAlbumTagBatchPreflightRequest {
  album_id: string;
  tracks: LocalAlbumTagBatchExpectedTrack[];
  update: LocalAlbumTagBatchUpdate;
}

export type LocalAlbumTagBatchBlockedReason =
  | "track_inspection_failed"
  | "cue_virtual";

export interface LocalAlbumTagBatchTrackPlan {
  uri: string;
  expected_revision: string;
  update: LocalTrackTagUpdate;
  needs_write: boolean;
  issue: LocalTrackTagsError | null;
}

export interface LocalAlbumTagBatchPlan {
  album_id: string;
  applicable: boolean;
  blocked_reason: LocalAlbumTagBatchBlockedReason | null;
  tracks: LocalAlbumTagBatchTrackPlan[];
  change_count: number;
  unchanged_count: number;
}

export type LocalAlbumTagBatchPreflightErrorCode =
  | "album_catalog_failure"
  | "album_not_found"
  | "membership_changed"
  | "duplicate_expected_uri"
  | "invalid_snapshot"
  | "revision_conflict";

export interface LocalAlbumTagBatchPreflightError {
  code: LocalAlbumTagBatchPreflightErrorCode;
  message?: string;
}
