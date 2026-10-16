export interface LocalItem {
  item_type: "directory" | "file";
  path: string;
  name: string;
  title?: string;
  artist?: string;
  album?: string;
  duration?: number;
}

export interface LocalAlbumTagPresence {
  evaluated_track_count: number;
  missing: { album: number; artist: number; album_artist: number; date: number; genre: number };
}

export interface LocalAlbum {
  id: string;
  title: string;
  artist: string;
  year?: string;
  genres?: string[];
  folder_path: string;
  source_id?: string | null;
  source_label?: string | null;
  track_count: number;
  discs: LocalAlbumDisc[];
  tag_presence?: LocalAlbumTagPresence | null;
}

export interface LocalAlbumDisc {
  number: number;
  label: string;
  folder_path: string;
  track_count: number;
}

export interface LocalAlbumDiscTracks {
  disc: LocalAlbumDisc;
  tracks: LocalItem[];
}
