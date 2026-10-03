import type { MediaLocator } from "./audio";

export interface PlaylistItemMetadata {
  title: string;
  artist: string;
  album: string;
  duration?: number | null;
}

export interface PlaylistItem {
  id: string;
  media_locator: MediaLocator;
  metadata: PlaylistItemMetadata;
}

export interface Playlist {
  id: string;
  name: string;
  created_at: number;
  updated_at: number;
  items: PlaylistItem[];
}
