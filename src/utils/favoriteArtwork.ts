import type { FavoriteAlbum } from "../types/favorite";
import type { LocalAlbum } from "../types/local";

export function sqliteFavoriteArtworkIdentity(
  favorite: Pick<FavoriteAlbum, "id" | "source" | "path_or_key">,
  sqliteBacked: boolean,
): string | null {
  if (!sqliteBacked || favorite.source !== "local" || !favorite.path_or_key.startsWith("sqlite-album://")) return null;
  return JSON.stringify([favorite.id, favorite.path_or_key]);
}

export function localAlbumArtworkIdentity(album: Pick<LocalAlbum, "id" | "folder_path">): string {
  return JSON.stringify([album.id, album.folder_path]);
}
