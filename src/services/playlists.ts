import { invoke } from "@tauri-apps/api/core";

import type { Playlist } from "../types/playlist";

export const playlistsService = {
  list: (): Promise<Playlist[]> => invoke<Playlist[]>("list_playlists"),
  create: (name: string): Promise<Playlist> => invoke<Playlist>("create_playlist", { name }),
  rename: (id: string, name: string): Promise<Playlist> =>
    invoke<Playlist>("rename_playlist", { id, name }),
  delete: (id: string): Promise<void> => invoke<void>("delete_playlist", { id }),
};
