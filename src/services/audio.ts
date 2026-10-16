import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { MpdStatusSnapshot, PlaybackStatus, PlaybackOptions, PlaybackOptionAction, TrackMetadata } from "../types/audio";
import { LocalItem, LocalAlbum, LocalAlbumDiscTracks } from "../types/local";
import type { OnlineCacheStatus } from "../utils/localArtwork";

import { diagnosticInvoke } from "../utils/frontendDiagnostics";
const invoke = <T>(command: string, args?: Record<string, unknown>): Promise<T> =>
  diagnosticInvoke<T>(nativeInvoke<T>, command, args);

const onlineArtworkRequest = (album: LocalAlbum) => ({
  album_id: album.id,
  title: album.title,
  artist: album.artist,
  year: album.year ?? null,
});

export const audioService = {
  getStatus: (): Promise<PlaybackStatus> => invoke<PlaybackStatus>("get_playback_status"),
  getMpdStatusSnapshot: (): Promise<MpdStatusSnapshot> =>
    invoke<MpdStatusSnapshot>("get_mpd_status_snapshot"),
  changePlaybackOption: (action: PlaybackOptionAction, expected: PlaybackOptions): Promise<PlaybackOptions> =>
    invoke<PlaybackOptions>("change_playback_option", { action, expected }),
  togglePlay: (): Promise<void> => invoke<void>("toggle_playback"),
  next: (): Promise<void> => invoke<void>("next_track"),
  previous: (): Promise<void> => invoke<void>("previous_track"),
  seek: (seconds: number): Promise<void> => invoke<void>("seek_playback", { seconds }),
  playUris: (uris: string[], startIndex: number): Promise<void> =>
    invoke<void>("play_uris", { uris, startIndex }),
  playTracks: (tracks: TrackMetadata[], startIndex: number): Promise<void> =>
    invoke<void>("play_tracks", { tracks, startIndex }),
  setVolume: (volume: number): Promise<void> => invoke<void>("set_volume", { volume }),
  getQueue: (): Promise<TrackMetadata[]> => invoke<TrackMetadata[]>("get_queue"),
  playQueueIndex: (index: number): Promise<void> => invoke<void>("play_queue_index", { index }),
  clearQueue: (): Promise<void> => invoke<void>("clear_queue"),
  setWindowTitle: (title: string): Promise<void> => invoke<void>("set_window_title", { title }),
  listLocalDirectory: (path: string): Promise<LocalItem[]> =>
    invoke<LocalItem[]>("list_local_directory", { path }),
  getLocalCover: (path: string): Promise<string | null> =>
    invoke<string | null>("get_local_cover", { path }),
  getOnlineAlbumCover: (album: LocalAlbum): Promise<string | null> =>
    invoke<string | null>("get_online_album_cover", { request: onlineArtworkRequest(album) }),
  getCachedOnlineAlbumCover: (album: LocalAlbum): Promise<string | null> =>
    invoke<string | null>("get_online_album_cover", { request: onlineArtworkRequest(album), cacheOnly: true }),
  getOnlineCoverCacheStatus: (album: LocalAlbum): Promise<OnlineCacheStatus> =>
    invoke<OnlineCacheStatus>("get_online_cover_cache_status", { request: onlineArtworkRequest(album) }),
  getLocalAlbums: (): Promise<LocalAlbum[]> => invoke<LocalAlbum[]>("get_local_albums"),
  getLocalAlbumTracks: (albumId: string): Promise<LocalAlbumDiscTracks[]> =>
    invoke<LocalAlbumDiscTracks[]>("get_local_album_tracks", { albumId }),
  resolveLocalLibraryPath: (path: string): Promise<string> =>
    invoke<string>("resolve_local_library_path", { path }),
  pickDirectory: (): Promise<string | null> => invoke<string | null>("pick_directory"),
  checkLocalLibrary: (): Promise<boolean> => invoke<boolean>("check_local_library"),
  rescanLibrary: (): Promise<void> => invoke<void>("rescan_library"),
};

export const LOCAL_LIBRARY_RECONCILED_EVENT = "local-library-reconciled";
