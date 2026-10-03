import type { PlexImageRef } from "./plex";

export interface FavoriteAlbum {
  id: string;
  source: "local" | "plex";
  title: string;
  artist: string;
  year?: string;
  thumb?: string | null;
  plex_image?: PlexImageRef | null;
  path_or_key: string;
  exists?: boolean;
}
