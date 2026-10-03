import type { MediaLocator } from "./audio";

export interface PlexImageRef {
  server_id: string;
  path: string;
}

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
  thumb?: PlexImageRef;
}

export interface PlexCollection {
  server_id: string;
  rating_key: string;
  title: string;
  child_count: number;
  thumb?: PlexImageRef;
}

export interface PlexTrack {
  rating_key: string;
  title: string;
  album_title?: string;
  artist?: string;
  thumb?: PlexImageRef | null;
  media_locator: MediaLocator;
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
  thumb?: PlexImageRef;
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
