import * as metadata from "../../src/utils/queueActions.ts";
// Unrelated artwork/navigation fixtures keep collection actions inert. Their own
// behavior is exercised with real hooks in collectionTools.test.mjs.
export const passiveTrackSelection = { useTrackSelection: () => ({ click: () => false, checked: () => false, checkbox: () => null, toolbar: null }) };
export const passiveQueueInsertion = {
  "./TrackSelection": passiveTrackSelection,
  "./QueueInsertionMenu": { useQueueInsertion: () => ({ open() {}, trigger: () => null, menu: null, notice: null }) },
  "../utils/queueActions": metadata,
  "../utils/queueAlbumLoader": { loadQueuedAlbum: async () => { throw new Error("unexpected album preparation in unrelated fixture"); }, loadLocalAlbumQueue: async () => { throw new Error("unexpected album preparation in unrelated fixture"); } },
};
