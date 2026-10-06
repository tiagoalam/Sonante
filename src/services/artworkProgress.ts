import { invoke } from "@tauri-apps/api/core";

export type ArtworkProgressResult = "local" | "embedded" | "online" | "ineligible" | "none";

export interface ArtworkProgressEntry {
  result: ArtworkProgressResult;
  online_complete: boolean;
  checked_at: number;
}

export interface ArtworkProgressChange {
  album_id: string;
  result: ArtworkProgressResult;
  online_complete: boolean;
}

export const artworkProgressService = {
  load: (): Promise<Record<string, ArtworkProgressEntry>> =>
    invoke<Record<string, ArtworkProgressEntry>>("get_artwork_enrichment_progress"),
  save: (changes: ArtworkProgressChange[]): Promise<void> =>
    invoke<void>("save_artwork_enrichment_progress", { changes }),
};
