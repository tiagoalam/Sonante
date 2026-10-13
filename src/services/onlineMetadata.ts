import { invoke } from "@tauri-apps/api/core";
import type { OnlineMetadataDetails, OnlineMetadataQuery, OnlineMetadataResult } from "../types/onlineMetadata";

export const onlineMetadataService = {
  details: (mbid: string): Promise<OnlineMetadataDetails> =>
    invoke("get_online_album_metadata_details", { mbid }),
  search: (request: OnlineMetadataQuery): Promise<OnlineMetadataResult> =>
    invoke("search_online_album_metadata", { request }),
};
