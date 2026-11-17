import { invoke } from "@tauri-apps/api/core";
import type {
  LocalAlbumTagBatchPlan,
  LocalAlbumTagBatchResult,
  LocalAlbumTagBatchPreflightRequest,
  LocalAlbumTagInspection,
  LocalTrackTags,
  LocalTrackTagUpdate,
  LocalTrackTagUpdateResult,
} from "../types/localTags";

export const localTagsService = {
  get: (path: string): Promise<LocalTrackTags> =>
    invoke<LocalTrackTags>("get_local_track_tags", { path }),

  update: (
    path: string,
    expectedRevision: string,
    patch: LocalTrackTagUpdate,
  ): Promise<LocalTrackTagUpdateResult> =>
    invoke<LocalTrackTagUpdateResult>("update_local_track_tags", {
      path,
      expectedRevision,
      patch,
    }),

  getAlbumInspection: (albumId: string): Promise<LocalAlbumTagInspection> =>
    invoke<LocalAlbumTagInspection>(albumId.startsWith("sqlite:")
      ? "sqlite_library_tag_album_inspection" : "get_local_album_tag_inspection", { albumId }),

  preflightAlbumBatch: (request: LocalAlbumTagBatchPreflightRequest): Promise<LocalAlbumTagBatchPlan> =>
    invoke<LocalAlbumTagBatchPlan>(request.album_id.startsWith("sqlite:")
      ? "sqlite_library_tag_album_preflight" : "preflight_local_album_tag_batch", { request }),

  applyAlbumBatch: (request: LocalAlbumTagBatchPreflightRequest): Promise<LocalAlbumTagBatchResult> =>
    invoke<LocalAlbumTagBatchResult>(request.album_id.startsWith("sqlite:")
      ? "sqlite_library_tag_album_apply" : "apply_local_album_tag_batch", { request }),
};
