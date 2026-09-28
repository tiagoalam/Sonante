export interface LocalItem {
  item_type: "directory" | "file";
  path: string;
  name: string;
  title?: string;
  artist?: string;
  album?: string;
  duration?: number;
}

export interface LocalAlbum {
  id: string;
  title: string;
  artist: string;
  year?: string;
  folder_path: string;
  track_count: number;
}
