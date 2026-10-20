import { ensureDirectOutputChoice } from "./config";
import { beginPlaybackIntent, configurePlaybackIntents, playbackIntentToken, type PlaybackIntent } from "../utils/playbackIntent";
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

configurePlaybackIntents(() => invoke<string>("admit_playback_intent"), token => invoke("cancel_playback_intent", { token }), token => invoke<boolean>("playback_intent_current", { token }));

async function requestedPlayback<T>(command:string,args:Record<string,unknown>,intent:PlaybackIntent):Promise<T>{
 const token=await playbackIntentToken(intent);await ensureDirectOutputChoice(intent);if(!intent.current())throw 'PLAYBACK_SUPERSEDED';return invoke<T>(command,{...args,intent:token});
}
async function ordinaryToggle():Promise<void>{
 const intent=beginPlaybackIntent(),token=await playbackIntentToken(intent),status=await invoke<PlaybackStatus>('get_playback_status');
 if(!intent.current())throw 'PLAYBACK_SUPERSEDED';const wantPlay=status.state!=='play';
 if(wantPlay)await ensureDirectOutputChoice(intent);if(!intent.current())throw 'PLAYBACK_SUPERSEDED';
 await invoke('toggle_playback',{intent:token,wantPlay});
}
export const audioService = {
  getStatus: (): Promise<PlaybackStatus> => invoke<PlaybackStatus>("get_playback_status"),
  getMpdStatusSnapshot: (): Promise<MpdStatusSnapshot> =>
    invoke<MpdStatusSnapshot>("get_mpd_status_snapshot"),
  changePlaybackOption: (action: PlaybackOptionAction, expected: PlaybackOptions): Promise<PlaybackOptions> =>
    invoke<PlaybackOptions>("change_playback_option", { action, expected }),
  togglePlay: (): Promise<void> => ordinaryToggle(),
  next: (): Promise<void> => requestedPlayback<void>("next_track",{},beginPlaybackIntent()),
  previous: (): Promise<void> => requestedPlayback<void>("previous_track",{},beginPlaybackIntent()),
  seek: (seconds: number): Promise<void> => invoke<void>("seek_playback", { seconds }),
  playUris: async (uris: string[], startIndex: number, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> =>
    requestedPlayback<void>("play_uris", { uris, startIndex },intent),
  playTracks: async (tracks: TrackMetadata[], startIndex: number, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> =>
    requestedPlayback<void>("play_tracks", { tracks, startIndex },intent),
  setDirectMute: async (request: { muted: boolean; token: string }): Promise<void> => {
    if(!request.muted){const intent=beginPlaybackIntent();await playbackIntentToken(intent);await ensureDirectOutputChoice(intent);if(!intent.current())throw 'PLAYBACK_SUPERSEDED';}
    return invoke<void>("set_direct_mute",request);
  },
  setSharedMute: (request: {muted:boolean;token:string}): Promise<void> => invoke<void>("set_shared_mute", request),
  setVolume: (volume: number): Promise<void> => invoke<void>("set_volume", { volume }),
  getQueue: (): Promise<TrackMetadata[]> => invoke<TrackMetadata[]>("get_queue"),
  playQueueIndex: async (index: number, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> => requestedPlayback<void>("play_queue_index", { index },intent),
  clearQueue: async (intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> => invoke<void>("clear_queue", { intent: await playbackIntentToken(intent) }),
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
