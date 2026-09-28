export interface PlexLibrary {
  key: string;
  title: string;
}

export interface PlexAlbum {
  rating_key: string;
  title: string;
  artist: string;
  artist_rating_key?: string;
  year?: number;
  thumb?: string;
}

export interface PlexCollection {
  rating_key: string;
  title: string;
  child_count: number;
  thumb?: string;
}

export interface PlexTrack {
  rating_key: string;
  title: string;
  album_title?: string;
  thumb?: string;
  track_index: number;
  duration_ms: number;
  play_uri: string;
}

export interface SelectedArtist {
  rating_key: string;
  name: string;
}

export interface PlexArtistResult {
  rating_key: string;
  name: string;
  thumb?: string;
}

export interface PlexSearchResults {
  artists: PlexArtistResult[];
  albums: PlexAlbum[];
  tracks: PlexTrack[];
}
