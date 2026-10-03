import { invoke } from "@tauri-apps/api/core";

import type {
  NewPlaylistItem,
  Playlist,
  PlaylistItemAvailability,
} from "../types/playlist";

export const isDuplicatePlaylistNameError = (cause: unknown): boolean =>
  String(cause).includes("playlist_name_duplicate");

export const playlistsService = {
  list: (): Promise<Playlist[]> => invoke<Playlist[]>("list_playlists"),
  create: (name: string): Promise<Playlist> => invoke<Playlist>("create_playlist", { name }),
  createWithItems: (name: string, items: NewPlaylistItem[]): Promise<Playlist> =>
    invoke<Playlist>("create_playlist_with_items", { name, items }),
  rename: (id: string, name: string): Promise<Playlist> =>
    invoke<Playlist>("rename_playlist", { id, name }),
  delete: (id: string): Promise<void> => invoke<void>("delete_playlist", { id }),
  addItem: (playlistId: string, item: NewPlaylistItem): Promise<Playlist> =>
    invoke<Playlist>("add_playlist_item", { playlistId, item }),
  addItems: (playlistId: string, items: NewPlaylistItem[]): Promise<Playlist> =>
    invoke<Playlist>("add_playlist_items", { playlistId, items }),
  removeItem: (playlistId: string, itemId: string): Promise<Playlist> =>
    invoke<Playlist>("remove_playlist_item", { playlistId, itemId }),
  reorderItems: (playlistId: string, orderedItemIds: string[]): Promise<Playlist> =>
    invoke<Playlist>("reorder_playlist_items", { playlistId, orderedItemIds }),
  resolveItems: (playlistId: string): Promise<PlaylistItemAvailability[]> =>
    invoke<PlaylistItemAvailability[]>("resolve_playlist_items", { playlistId }),
  play: (playlistId: string, startItemId?: string, shuffle = false): Promise<{ skipped_count: number }> =>
    invoke<{ skipped_count: number }>("play_playlist", { playlistId, startItemId, shuffle }),
};
