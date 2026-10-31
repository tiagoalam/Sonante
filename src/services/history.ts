import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type { TrackMetadata } from "../types/audio";
import type { PlayerPlexNavigation } from "../types/plex";
export interface HistoryEntry { id: string; played_at: number; track: TrackMetadata }
export interface HistoryFile { version: number; enabled: boolean; entries: HistoryEntry[] }
export const historyService = {
  subscribe: (changed: () => void) => listen<void>("listening-history-changed", changed),
  list: () => invoke<HistoryFile>("listening_history"),
  configure: (enabled?: boolean, clear = false) => invoke<HistoryFile>("listening_history", { enabled, clear }),
  plexNavigation: (id: string) => invoke<PlayerPlexNavigation["navigation"]>("history_plex_navigation", { id }),
};
