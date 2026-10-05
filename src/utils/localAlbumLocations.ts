import type { LocalAlbum } from "../types/local";

export interface AlbumLocationSource {
  label: string | null;
  path: string;
}

export function albumLocationSources(album: LocalAlbum): AlbumLocationSource[] {
  if (album.discs.length <= 1) {
    return [{ label: null, path: album.folder_path }];
  }
  return album.discs.map((disc) => ({ label: disc.label, path: disc.folder_path }));
}
