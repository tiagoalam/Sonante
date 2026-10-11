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
