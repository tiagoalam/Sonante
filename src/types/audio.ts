export type MediaLocator =
  | { kind: "local"; uri: string }
  | { kind: "plex"; server_id: string; part_key: string; file_path?: string | null };

export interface TrackMetadata {
  title: string;
  artist: string;
  album: string;
  thumb?: string | null;
  media_locator?: MediaLocator | null;
  uri?: string;
  duration?: number;
}

export interface AudioDevice {
  id: string;
  name: string;
}

export interface PlaybackStatus {
  state: string;
  elapsed: number;
  duration: number;
  audio_format: string;
  current_file: string;
  title: string;
  artist: string;
  album: string;
  thumb?: string | null;
  volume: number;
  is_updating: boolean; // <-- Necessário para o ícone de indexação/sincronização
}

export type MpdUnavailableReason =
  | "process_exited"
  | "socket_unavailable"
  | "protocol_unavailable"
  | "startup_failed";

export type MpdHealth =
  | { state: "starting" }
  | { state: "available" }
  | { state: "unavailable"; reason: MpdUnavailableReason }
  | { state: "stopping" };

export interface MpdStatusSnapshot {
  health: MpdHealth;
  playback: PlaybackStatus | null;
}

export interface QueueTrack {
  id: number;
  pos: number;
  file: string;
  title?: string;
  artist?: string;
  album?: string;
  duration?: number;
}
