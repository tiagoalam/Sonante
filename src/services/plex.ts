import { invoke } from "@tauri-apps/api/core";
import { PlexLibrary, PlexAlbum, PlexCollection, PlexTrack } from "../types/plex";

export const plexService = {
  getLibraries: (): Promise<PlexLibrary[]> =>
    invoke<PlexLibrary[]>("get_plex_libraries"),

  getAlbums: (sectionKey: string, sortBy: string): Promise<PlexAlbum[]> =>
    invoke<PlexAlbum[]>("get_plex_albums", { sectionKey, sortBy }),

  getCollections: (sectionKey: string): Promise<PlexCollection[]> =>
    invoke<PlexCollection[]>("get_plex_collections", { sectionKey }),

  getCollectionAlbums: (ratingKey: string): Promise<PlexAlbum[]> =>
    invoke<PlexAlbum[]>("get_collection_albums", { ratingKey }),

  getArtistAlbums: (ratingKey: string): Promise<PlexAlbum[]> =>
    invoke<PlexAlbum[]>("get_artist_albums", { ratingKey }),

  getArtistTopTracks: (ratingKey: string): Promise<PlexTrack[]> =>
    invoke<PlexTrack[]>("get_artist_top_tracks", { ratingKey }),

  search: (query: string, sectionKey?: string): Promise<PlexSearchResults> =>
    invoke<PlexSearchResults>("search_plex", { query, sectionKey }),
  
  getAlbumTracks: (ratingKey: string): Promise<PlexTrack[]> =>
    invoke<PlexTrack[]>("get_album_tracks", { ratingKey }),
};
