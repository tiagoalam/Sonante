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
  artist?: string;
  thumb?: string | null;
  play_uri: string;
  duration?: number;
  duration_ms?: number;
  index?: number;
  track_index?: number;
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

export interface PlexPin {
  id: number;
  code: string;
  expires_at: string;
  auth_url: string;
}

export interface PlexConnection {
  uri: string;
  local: boolean;
  address: string;
  port: number;
}

export interface PlexServerResource {
  name: string;
  client_identifier: string;
  connections: PlexConnection[];
  chosen_uri: string;
}
