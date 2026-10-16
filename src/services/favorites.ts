import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { FavoriteAlbum } from "../types/favorite";

import { diagnosticInvoke } from "../utils/frontendDiagnostics";
import { sharePendingRequest } from "../utils/sharePendingRequest";
const invoke = <T>(command: string, args?: Record<string, unknown>): Promise<T> =>
  diagnosticInvoke<T>(nativeInvoke<T>, command, args);
const getFavorites = sharePendingRequest(() => invoke<FavoriteAlbum[]>("get_favorites"));

export const favoritesService = {
  getFavorites,
  toggleFavorite: (album: FavoriteAlbum): Promise<boolean> =>
    invoke<boolean>("toggle_favorite", { album }),
};
