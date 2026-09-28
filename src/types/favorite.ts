export interface FavoriteAlbum {
  id: string;
  source: "local" | "plex";
  title: string;
  artist: string;
  year?: string;
  thumb?: string | null;
  path_or_key: string;
  exists?: boolean;
}
