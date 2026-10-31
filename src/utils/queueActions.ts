import type { TrackMetadata } from "../types/audio";
import type { LocalAlbum, LocalItem } from "../types/local";
import type { PlexAlbum, PlexTrack } from "../types/plex";
import { plexTrackArtist, plexTrackDurationSeconds } from "./plexTrackMetadata.ts";

export interface QueueMenuRequest {
  track: TrackMetadata;
  album?: () => Promise<TrackMetadata[]>;
  current?: () => boolean;
}
export function localQueueTrack(track: LocalItem, album?: LocalAlbum | null, cover?: string | null): TrackMetadata {
  return { title: track.title || track.name, artist: track.artist || album?.artist || "", album: track.album || album?.title || "",
    media_locator: { kind: "local", uri: track.path }, duration: track.duration, thumb: cover ?? null };
}
export function plexQueueTrack(track: PlexTrack, album?: PlexAlbum): TrackMetadata {
  return { title: track.title, artist: plexTrackArtist(track, album?.artist), album: track.album_title || album?.title || "",
    media_locator: track.media_locator, duration: plexTrackDurationSeconds(track), plex_image: track.thumb || album?.thumb || null };
}
