/** Discoveries only: embedded thumbnails, never catalog IDs or playback URIs. */
export interface AlbumDiscovery {
  provisional_group_key: string;
  source_id: string;
  album: string | null;
  artist: string | null;
  discs: string[];
  observed_tracks: number;
  image?: string | null;
}

export interface LocalImportPreview {
  operation_id: string;
  provisional: true;
  playable: false;
  complete: false;
  sampled_tracks: number;
  sample_limit: number;
  albums: AlbumDiscovery[];
}

export type PreviewAlbum = Pick<AlbumDiscovery, "provisional_group_key" | "source_id" | "album" | "artist" | "image">;
