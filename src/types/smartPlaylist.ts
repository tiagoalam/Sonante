import type { LocalGenreFamily } from '../utils/localGenreFamilies';
import type { PlexImageRef } from './plex';
import type { TrackMetadata } from './audio';
export type SmartSource = {
    kind: 'local';
} | {
    kind: 'plex';
    server_id: string;
    library_id: string;
} | { kind: 'navidrome'; account_id: string };
export type SmartSort = 'album' | 'artist' | 'added_newest' | 'added_oldest' | 'year_newest' | 'year_oldest';
export interface SmartRules {
    source: SmartSource;
    genre: string | null;
    genre_family?: LocalGenreFamily | null;
    track_mode?: 'varied' | 'album';
    artist: string | null;
    year_from: number | null;
    year_to: number | null;
    added_days: number | null;
    favorites_only: boolean;
    sort: SmartSort;
    limit: number;
}
export interface SmartPlaylist {
    id: string;
    name: string;
    created_at: number;
    updated_at: number;
    rules: SmartRules;
}
export interface SmartAlbum {
    image?: PlexImageRef | null;
    id: string;
    title: string;
    artist: string;
    genres: string[];
    year: number | null;
    added_at: number | null;
}
export interface SmartPreview {
    tracks: TrackMetadata[];
    matched_albums: number;
    limit_reached: boolean;
}
export interface SmartSources {
    local: boolean;
    plex_server_id: string | null;
    navidrome_account_id?: string | null;
}

export interface SmartSelection { albums: SmartAlbum[]; matched_albums: number }
