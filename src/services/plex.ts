import { invoke } from "@tauri-apps/api/core";
import { requireCompletePlexCatalog } from "../utils/plexCatalog";
import type { CurrentMedia } from "../types/audio";
import type { PlexDiscoverPage, PlexDiscoverRecent, PlexDiscoverHub, PlexDiscoverArtistContent, PlayerPlexNavigation, PlexLibraryMixPage, PlexMixYears, PlexMixStyles, PlexStyleSelection } from "../types/plex";

import {
  PlexLibrary,
  PlexAlbum,
  PlexAlbumCatalog,
  PlexCollection,
  PlexTrack,
  PlexSearchResults,
  PlexPin,
  PlexServerResource,
  PlexImageRef,
} from "../types/plex";

export const plexService = {
  getLibraryMixStyles: (serverId: string, sectionKey: string, language: string): Promise<PlexMixStyles> =>
    invoke("get_plex_library_mix_styles", { serverId, sectionKey, language }),
  getLibraryMixYears: (serverId: string, sectionKey: string, language: string): Promise<PlexMixYears> =>
    invoke("get_plex_library_mix_years", { serverId, sectionKey, language }),
  getLibraryMix: (serverId: string, sectionKey: string, language: string, limit: number, decade?: number, style?: PlexStyleSelection): Promise<PlexLibraryMixPage> =>
    invoke("get_plex_library_mix", { serverId, sectionKey, language, limit, decade: decade ?? null, style: style ?? null }),
  getDiscover: (serverId: string, sectionKey: string, language: string): Promise<PlexDiscoverPage> =>
    invoke("get_plex_discover", { serverId, sectionKey, language }),
  getDiscoverRecent: (serverId: string, sectionKey: string, language: string, windowSize: number): Promise<PlexDiscoverRecent> =>
    invoke("get_plex_discover_recent", { serverId, sectionKey, language, windowSize }),
  getDiscoverChildren: (serverId: string, sectionKey: string, kind: string, ratingKey: string, language: string): Promise<PlexDiscoverHub> =>
    invoke("get_plex_discover_children", { serverId, sectionKey, kind, ratingKey, language }),
  getDiscoverArtist: (serverId: string, sectionKey: string, ratingKey: string, language: string): Promise<PlexDiscoverArtistContent> =>
    invoke("get_plex_discover_artist", { serverId, sectionKey, ratingKey, language }),
  resolvePlayerNavigation: (media: Extract<CurrentMedia, { kind: "plex" }>): Promise<PlayerPlexNavigation> =>
    invoke<PlayerPlexNavigation>("resolve_player_plex_navigation", { media }),
  getLibraries: (): Promise<PlexLibrary[]> =>
    invoke<PlexLibrary[]>("get_plex_libraries"),

  getAlbums: (sectionKey: string, sortBy: string): Promise<PlexAlbum[]> =>
    invoke<PlexAlbumCatalog>("get_plex_albums", { sectionKey, sortBy })
      .then(catalog => requireCompletePlexCatalog(catalog, sectionKey)),

  getCollections: (sectionKey: string): Promise<PlexCollection[]> =>
    invoke<PlexCollection[]>("get_plex_collections", { sectionKey }),

  getCollectionAlbums: (ratingKey: string): Promise<PlexAlbum[]> =>
    invoke<PlexAlbum[]>("get_collection_albums", { ratingKey }),

  getArtistPortrait: (serverId: string, ratingKey: string): Promise<PlexImageRef | null> =>
    invoke<PlexImageRef | null>("get_artist_portrait", { serverId, ratingKey }),

  getArtistAlbums: (ratingKey: string): Promise<PlexAlbum[]> =>
    invoke<PlexAlbum[]>("get_artist_albums", { ratingKey }),

  getArtistTopTracks: (ratingKey: string): Promise<PlexTrack[]> =>
    invoke<PlexTrack[]>("get_artist_top_tracks", { ratingKey }),

  search: (query: string, sectionKey?: string): Promise<PlexSearchResults> =>
    invoke<PlexSearchResults>("search_plex", { query, sectionKey }),
 
  createPin: (): Promise<PlexPin> => invoke<PlexPin>("plex_create_pin"),
  
  checkPin: (pinId: number): Promise<string | null> =>
    invoke<string | null>("plex_check_pin", { pinId }),
  
  getServers: (authToken: string): Promise<PlexServerResource[]> =>
    invoke<PlexServerResource[]>("plex_get_servers", { authToken }),
  
  openExternalUrl: (url: string): Promise<void> =>
    invoke<void>("open_external_url", { url }),
 
  getAlbumTracks: (ratingKey: string): Promise<PlexTrack[]> =>
    invoke<PlexTrack[]>("get_album_tracks", { ratingKey }),

  getImage: (image: PlexImageRef): Promise<ArrayBuffer> =>
    invoke<ArrayBuffer>("get_plex_image", { image }),
};
