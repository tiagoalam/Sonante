import type { CurrentMedia, MediaLocator } from "./audio";

export interface PlayerPlexNavigation {
  media: Extract<CurrentMedia, { kind: "plex" }>;
  navigation: {
    album: PlexAlbum;
    artist_rating_key: string;
    artist_name: string;
  };
}

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
  genres?: string[];
  thumb?: PlexImageRef;
  // Session catalog context; rating_key remains the original Plex identity.
  server_id?: string;
  library_id?: string;
  library_title?: string;
}

export interface PlexAlbumCatalog {
  server_id: string;
  library_id: string;
  completeness: "complete" | "partial" | "unknown";
  total: number;
  albums: PlexAlbum[];
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
  server_id?: string; library_id?: string; thumb?: PlexImageRef | null;
}

export interface PlexDiscoverArtistContent {
  server_id: string; library_id: string; artist_id: string;
  albums: PlexAlbum[]; top_tracks: PlexTrack[];
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

export interface PlexDiscoverItem {
  server_id: string; library_id: string; library_title?: string;
  kind: "album" | "artist" | "collection" | "track";
  rating_key: string; title: string; subtitle: string;
  thumb: PlexImageRef | null; album: PlexAlbum | null;
  added_at?: number | null; last_viewed_at?: number | null;
  // Presentation only: members retain original server/library/rating identities.
  presentation_key?: string; artist_members?: PlexDiscoverItem[];
}
export interface PlexDiscoverHub {
  artist_recommendation?: boolean; artist_candidates?: PlexDiscoverItem[];
  presentation?: "artist_suggestions";
  context?: string | null; source_id?: string; library_id?: string; library_title?: string;
  recent_family?: "added" | "played" | "played_artists" | null; dates_missing?: boolean; source_order?: number;
  id: string; title: string; items: PlexDiscoverItem[]; partial: boolean;
}
export interface PlexDiscoverPage {
  language: string;
  server_id: string; library_id: string; hubs: PlexDiscoverHub[]; partial: boolean;
}
export interface PlexDiscoverRecent {
  language: string; server_id: string; library_id: string;
  items: PlexDiscoverItem[]; partial: boolean; dates_missing: boolean;
}
export interface PlexMixTrack extends PlexTrack {
  server_id: string; library_id: string; library_title?: string;
}
export interface PlexLibraryMixPage {
  server_id: string; library_id: string; language: string;
  tracks: PlexMixTrack[]; partial: boolean; has_more: boolean;
}

export interface PlexMixYears {
  server_id: string; library_id: string; language: string; years: number[]; partial: boolean;
}

export interface PlexStyleSelection { field: "artist.style" | "album.style" | "track.style"; ids: string[] }
export interface PlexMixStyles { server_id: string; library_id: string; language: string; field: PlexStyleSelection["field"]; choices: {id: string; title: string}[]; partial: boolean }
