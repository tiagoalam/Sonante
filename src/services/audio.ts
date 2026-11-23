import { ensureDirectOutputChoice } from "./config";
import { createLocalCoverBatch, type LocalCoverReply } from "../utils/localCoverBatch";
import { localSourceReadQueue as runLocalSourceRead } from "../utils/localSourceReadQueue";
import { beginPlaybackIntent, configurePlaybackIntents, playbackIntentToken, type PlaybackIntent } from "../utils/playbackIntent";
import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { MpdStatusSnapshot, PlaybackStatus, PlaybackOptions, PlaybackOptionAction, TrackMetadata } from "../types/audio";
import type { CurrentMedia } from "../types/audio";
import { LocalItem, LocalAlbum, LocalAlbumDiscTracks } from "../types/local";
import type { LocalSourceAvailabilityObservation, LocalSourceAvailabilityRow } from "../types/local";
import type { OnlineCacheStatus } from "../utils/localArtwork";
import type { LyricsQuery, LegacyLyricsCacheEntry, PersistedLyricsChoice } from "../utils/trackLyrics";

import { diagnosticInvoke } from "../utils/frontendDiagnostics";
import { isSqliteCatalogEnabled } from "../utils/sqliteExperimentalMode";
const invoke = <T>(command: string, args?: Record<string, unknown>): Promise<T> =>
  diagnosticInvoke<T>(nativeInvoke<T>, command, args);

export type LocalLyricsLookup =
  | { kind: "found"; content: string }
  | { kind: "not_found" }
  | { kind: "not_eligible"; reason: string }
  | { kind: "read_failed"; reason: string }
  | { kind: "invalid_content"; reason: string };
export interface OnlineLyricsQuery extends LyricsQuery { cacheKey?: string }
export type OnlineLyricsLookup =
  | { kind: "records"; records: import("../utils/trackLyrics").LyricsRecord[]; persisted?: boolean }
  | { kind: "not_found" | "offline" | "timeout" | "rate_limited" | "service_unavailable" | "invalid_response" };
const rejectUnresolvedSqliteLocators = (uris: readonly string[]): void => {
  if (uris.some(uri => typeof uri === "string" && (uri.startsWith("sqlite-track://") || uri.startsWith("sqlite-cue://")))) {
    throw new Error("SQLITE_LOCAL_PLAYBACK_NOT_CONNECTED");
  }
};
const readLocalCover = createLocalCoverBatch(paths =>
  runLocalSourceRead(() => invoke<LocalCoverReply[]>("get_local_covers", { paths })));


const onlineArtworkRequest = (album: LocalAlbum) => ({
  album_id: album.id,
  title: album.title,
  artist: album.artist,
  year: album.year ?? null,
});

configurePlaybackIntents(() => invoke<string>("admit_playback_intent"), token => invoke("cancel_playback_intent", { token }), token => invoke<boolean>("playback_intent_current", { token }));

async function requestedPlayback<T>(command:string,args:Record<string,unknown>,intent:PlaybackIntent):Promise<T>{
 const token=await playbackIntentToken(intent);
 return runLocalSourceRead.control(async()=>{
  if(!intent.current())throw 'PLAYBACK_SUPERSEDED';
  await ensureDirectOutputChoice(intent);if(!intent.current())throw 'PLAYBACK_SUPERSEDED';
  return invoke<T>(command,{...args,intent:token});
 });
}
async function ordinaryToggle():Promise<void>{
 const intent=beginPlaybackIntent(),token=await playbackIntentToken(intent);
 return runLocalSourceRead.control(async()=>{
  if(!intent.current())throw 'PLAYBACK_SUPERSEDED';
  const status=await invoke<PlaybackStatus>('get_playback_status');
  if(!intent.current())throw 'PLAYBACK_SUPERSEDED';const wantPlay=status.state!=='play';
  if(wantPlay)await ensureDirectOutputChoice(intent);if(!intent.current())throw 'PLAYBACK_SUPERSEDED';
  await invoke('toggle_playback',{intent:token,wantPlay});
 });
}
// This decoder accepts observations only. Reject the entire payload on any
// malformed row; never drop a source or turn missing evidence into Available.
function decodeLocalSourceAvailability(value: unknown): LocalSourceAvailabilityObservation | null {
  const record = (item: unknown): item is Record<string, unknown> =>
    typeof item === "object" && item !== null && !Array.isArray(item);
  const keys = (item: Record<string, unknown>, expected: readonly string[]) =>
    Object.keys(item).length === expected.length && expected.every(key => Object.prototype.hasOwnProperty.call(item, key));
  const safeLabel = (item: unknown): item is string => typeof item === "string"
    && item.length > 0 && item.length <= 255 && item !== "." && item !== ".."
    && !/[\/\\\u0000-\u001f\u007f]/.test(item) && !/^[a-z][a-z0-9+.-]*:/i.test(item);
  if (!record(value) || !keys(value, ["admission", "sources", "observed_conflict"])
    || value.admission !== "not_product_admission" || typeof value.observed_conflict !== "boolean"
    || !Array.isArray(value.sources) || value.sources.length > 65_536) return null;
  const sources: LocalSourceAvailabilityRow[] = [];
  const ids = new Set<string>();
  for (const row of value.sources) {
    if (!record(row) || !keys(row, ["source_id", "label", "status", "reason"])
      || !safeLabel(row.source_id) || !safeLabel(row.label) || ids.has(row.source_id)) return null;
    ids.add(row.source_id);
    const identity = { source_id: row.source_id, label: row.label };
    if (row.status === "available" && row.reason === null) {
      sources.push({ ...identity, status: "available", reason: null });
    } else if (row.status === "unavailable"
      && (row.reason === "not_found" || row.reason === "permission_denied" || row.reason === "io_error")) {
      sources.push({ ...identity, status: "unavailable", reason: row.reason });
    } else if (row.status === "identity_conflict" && row.reason === "origin_mismatch" && value.observed_conflict) {
      sources.push({ ...identity, status: "identity_conflict", reason: "origin_mismatch" });
    } else { return null; }
  }
  return { admission: "not_product_admission", sources, observed_conflict: value.observed_conflict };
}

export const audioService = {
  getStatus: (): Promise<PlaybackStatus> => invoke<PlaybackStatus>("get_playback_status"),
  getCurrentLocalLyrics: (media: CurrentMedia): Promise<LocalLyricsLookup> =>
    invoke<LocalLyricsLookup>("get_current_local_lyrics", { media }),
  getLyricsChoice: (media: CurrentMedia): Promise<PersistedLyricsChoice | null> =>
    invoke<PersistedLyricsChoice | null>("get_lyrics_choice", { media }),
  setLyricsChoice: (media: CurrentMedia, choice: PersistedLyricsChoice): Promise<void> =>
    invoke<void>("set_lyrics_choice", { media, choice }),
  migrateLegacyLyricsCache: async (entries: LegacyLyricsCacheEntry[]): Promise<boolean> => {
    try { await invoke<void>("migrate_legacy_lyrics_cache", { entries }); return true; } catch { return false; }
  },
  searchOnlineLyrics: (query: OnlineLyricsQuery, refresh = false): Promise<OnlineLyricsLookup> =>
    // Track metadata is intentionally excluded from generic IPC diagnostics.
    nativeInvoke<OnlineLyricsLookup>("search_lrclib", { query, refresh }),
  getMpdStatusSnapshot: (): Promise<MpdStatusSnapshot> =>
    invoke<MpdStatusSnapshot>("get_mpd_status_snapshot"),
  changePlaybackOption: (action: PlaybackOptionAction, expected: PlaybackOptions): Promise<PlaybackOptions> =>
    invoke<PlaybackOptions>("change_playback_option", { action, expected }),
  togglePlay: (): Promise<void> => ordinaryToggle(),
  next: (): Promise<void> => requestedPlayback<void>("next_track",{},beginPlaybackIntent()),
  previous: (): Promise<void> => requestedPlayback<void>("previous_track",{},beginPlaybackIntent()),
  seek: (seconds: number): Promise<void> => invoke<void>("seek_playback", { seconds }),
  playUris: async (uris: string[], startIndex: number, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> => {
    rejectUnresolvedSqliteLocators(uris);
    return requestedPlayback<void>("play_uris", { uris, startIndex },intent);
  },
  playTracks: async (tracks: TrackMetadata[], startIndex: number, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> => {
    rejectUnresolvedSqliteLocators(tracks.map(track => track.uri ?? ""));
    return requestedPlayback<void>("play_tracks", { tracks, startIndex },intent);
  },
  setDirectMute: async (request: { muted: boolean; token: string }): Promise<void> => {
    if(!request.muted){const intent=beginPlaybackIntent();await playbackIntentToken(intent);await ensureDirectOutputChoice(intent);if(!intent.current())throw 'PLAYBACK_SUPERSEDED';}
    return invoke<void>("set_direct_mute",request);
  },
  setSharedMute: (request: {muted:boolean;token:string}): Promise<void> => invoke<void>("set_shared_mute", request),
  setVolume: (volume: number): Promise<void> => invoke<void>("set_volume", { volume }),
  getQueueEditSnapshot: () => runLocalSourceRead.control(() => invoke<{ tracks: TrackMetadata[]; revision: string }>("get_queue_edit_snapshot")),
  editQueue: async (revision: string, action: { kind: "move"; from: number; to: number } | { kind: "next" | "remove"; index: number }, intent: PlaybackIntent): Promise<void> => {
    const token = await playbackIntentToken(intent);
    return runLocalSourceRead.control(() => { if (!intent.current()) throw "PLAYBACK_SUPERSEDED"; return invoke<void>("edit_queue", { revision, action, intent: token }); });
  },
  enqueueTracks: async (tracks: TrackMetadata[], placement: "next" | "end", intent: PlaybackIntent): Promise<number> => {
    rejectUnresolvedSqliteLocators(tracks.map(track => track.uri ?? ""));
    const token = await playbackIntentToken(intent);
    return runLocalSourceRead.control(() => {
      if (!intent.current()) throw "PLAYBACK_SUPERSEDED";
      // Insertion does not open the DAC or request Direct gain-policy changes.
      return invoke<number>("enqueue_tracks", { tracks, placement, intent: token });
    });
  },
  getQueue: (): Promise<TrackMetadata[]> => invoke<TrackMetadata[]>("get_queue"),
  playQueueIndex: async (index: number, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> => requestedPlayback<void>("play_queue_index", { index },intent),
  clearQueue: async (intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> => {
    const token = await playbackIntentToken(intent);
    return runLocalSourceRead.control(() => {
      if (!intent.current()) throw "PLAYBACK_SUPERSEDED";
      return invoke<void>("clear_queue", { intent: token });
    });
  },
  setWindowTitle: (title: string): Promise<void> => invoke<void>("set_window_title", { title }),
  listLocalDirectory: (path: string): Promise<LocalItem[]> =>
    invoke<LocalItem[]>("list_local_directory", { path }),
  getLocalCover: readLocalCover,
  // Enqueue the restored player's one focal read immediately, ahead of the
  // browser's passive catalog load. It shares the same serial source queue
  // and native admission; no parallel worker or privilege bypass is added.
  getPlayerLocalCover: (path: string): Promise<string | null> =>
    runLocalSourceRead(() => invoke<string | null>("get_local_cover", { path })),
  getOnlineAlbumCover: (album: LocalAlbum): Promise<string | null> =>
    invoke<string | null>("get_online_album_cover", { request: onlineArtworkRequest(album) }),
  getCachedOnlineAlbumCover: (album: LocalAlbum): Promise<string | null> =>
    invoke<string | null>("get_online_album_cover", { request: onlineArtworkRequest(album), cacheOnly: true }),
  getOnlineCoverCacheStatus: (album: LocalAlbum): Promise<OnlineCacheStatus> =>
    invoke<OnlineCacheStatus>("get_online_cover_cache_status", { request: onlineArtworkRequest(album) }),
  getLocalSourceAvailability: (): Promise<LocalSourceAvailabilityObservation> => runLocalSourceRead(async () => {
    const observation = decodeLocalSourceAvailability(await invoke<unknown>("get_local_source_availability"));
    if (!observation) throw new Error("LOCAL_SOURCE_AVAILABILITY_UNCONFIRMED");
    return observation;
  }),
  searchLocalTracks: (query: string): Promise<{ tracks: LocalItem[]; limited: boolean }> => {
    const read = () => invoke<{ tracks: LocalItem[]; limited: boolean }>(
      isSqliteCatalogEnabled() ? "sqlite_library_search_tracks" : "search_local_tracks",
      { query },
    );
    // SQLite catalog reads do not touch MPD-managed roots and must not queue
    // behind the legacy filesystem-read admission path.
    return isSqliteCatalogEnabled() ? read() : runLocalSourceRead(read);
  },
  getLocalAlbums: (): Promise<LocalAlbum[]> => {
    const read = () => invoke<LocalAlbum[]>(
      isSqliteCatalogEnabled() ? "sqlite_library_local_albums" : "get_local_albums",
    );
    return isSqliteCatalogEnabled() ? read() : runLocalSourceRead(read);
  },
  getLocalAlbumTracks: (albumId: string): Promise<LocalAlbumDiscTracks[]> =>
    invoke<LocalAlbumDiscTracks[]>(isSqliteCatalogEnabled() ? "sqlite_library_local_album_tracks" : "get_local_album_tracks", { albumId }),
  resolveLocalLibraryPath: (path: string): Promise<string> =>
    invoke<string>("resolve_local_library_path", { path }),
  pickDirectory: (): Promise<string | null> => invoke<string | null>("pick_directory"),
  checkLocalLibrary: (sourceId?: string, expectedFolders?: readonly string[]): Promise<boolean> =>
    invoke<boolean>("check_local_library", sourceId === undefined && expectedFolders === undefined ? undefined : { sourceId, expectedFolders }),
  rescanLibrary: (): Promise<void> => invoke<void>("rescan_library"),
};

export const LOCAL_LIBRARY_RECONCILED_EVENT = "local-library-reconciled";
