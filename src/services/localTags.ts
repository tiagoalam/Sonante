import { invoke } from "@tauri-apps/api/core";
import type {
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
};
