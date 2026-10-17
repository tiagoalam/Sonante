import { beginPlaybackIntent, isPlaybackSuperseded, type PlaybackIntent } from "./playbackIntent.ts";
import type { PlexAlbum, PlexTrack } from "../types/plex.ts";
import type { TrackMetadata } from "../types/audio.ts";

export async function playPlexAlbum(album: PlexAlbum, load: (key: string) => Promise<PlexTrack[]>,
  play: (tracks: TrackMetadata[], index: number, intent?: PlaybackIntent) => Promise<void>, current: () => boolean, intent: PlaybackIntent = beginPlaybackIntent()): Promise<void> {
  if (!current()) return;
  try {
  const tracks = await load(album.rating_key);
  if (!current() || !intent.current()) return;
  if (album.server_id && tracks.some(track => track.media_locator.kind !== "plex" || track.media_locator.server_id !== album.server_id)) throw new Error("PLEX_DISCOVER_CONTEXT");
  const metadata = tracks.map(track => ({ title: track.title, artist: album.artist, album: album.title,
    plex_image: track.thumb || album.thumb || null, media_locator: track.media_locator,
    duration: track.duration_ms ? track.duration_ms / 1000 : undefined }));
  if (metadata.length) await play(metadata, 0, intent);
  } catch (error) { if (!current() || isPlaybackSuperseded(error) || !await intent.confirmCurrent()) return; throw error; }
}
