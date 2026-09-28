import { invoke } from "@tauri-apps/api/core";
import { FavoriteAlbum } from "../types/favorite";

export const favoritesService = {
  getFavorites: (): Promise<FavoriteAlbum[]> => invoke<FavoriteAlbum[]>("get_favorites"),
  toggleFavorite: (album: FavoriteAlbum): Promise<boolean> =>
    invoke<boolean>("toggle_favorite", { album }),
};
