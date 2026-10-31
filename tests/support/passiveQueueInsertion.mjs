import * as metadata from "../../src/utils/queueActions.ts";
export const passiveQueueInsertion = {
  "./QueueInsertionMenu": { useQueueInsertion: () => ({ open() {}, trigger: () => null, menu: null, notice: null }) },
  "../utils/queueActions": metadata,
  "../utils/queueAlbumLoader": { loadQueuedAlbum: async () => { throw new Error("unexpected album preparation in unrelated fixture"); } },
};
