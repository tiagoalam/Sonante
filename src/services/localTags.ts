import { invoke } from "@tauri-apps/api/core";
import type {
  LocalAlbumTagBatchPlan,
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
    invoke<LocalAlbumTagInspection>("get_local_album_tag_inspection", { albumId }),

  preflightAlbumBatch: (request: LocalAlbumTagBatchPreflightRequest): Promise<LocalAlbumTagBatchPlan> =>
    invoke<LocalAlbumTagBatchPlan>("preflight_local_album_tag_batch", { request }),
};
