import { invoke } from "@tauri-apps/api/core";
import { MpdStatusSnapshot, PlaybackStatus, TrackMetadata } from "../types/audio";
import { LocalItem, LocalAlbum } from "../types/local";
import type { OnlineCacheStatus } from "../utils/localArtwork";

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
  getOnlineCoverCacheStatus: (album: LocalAlbum): Promise<OnlineCacheStatus> =>
    invoke<OnlineCacheStatus>("get_online_cover_cache_status", { request: onlineArtworkRequest(album) }),
  getLocalAlbums: (): Promise<LocalAlbum[]> => invoke<LocalAlbum[]>("get_local_albums"),
  resolveLocalLibraryPath: (path: string): Promise<string> =>
    invoke<string>("resolve_local_library_path", { path }),
  pickDirectory: (): Promise<string | null> => invoke<string | null>("pick_directory"),
  rescanLibrary: (): Promise<void> => invoke<void>("rescan_library"),
};
