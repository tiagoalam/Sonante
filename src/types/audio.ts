import type { PlexImageRef } from "./plex";

export type MediaLocator =
  | { kind: "local"; uri: string }
  | {
      kind: "plex";
      server_id: string;
      part_key: string;
      rating_key?: string | null;
      file_path?: string | null;
    };

export type CurrentMedia =
  | { kind: "local"; uri: string; queue_index: number }
  | { kind: "plex"; server_id: string; part_key: string; queue_index: number };

export interface TrackMetadata {
  title: string;
  artist: string;
  album: string;
  thumb?: string | null;
  plex_image?: PlexImageRef | null;
  media_locator?: MediaLocator | null;
  uri?: string;
  duration?: number;
}

export interface AudioDevice {
  id: string;
  name: string;
}

export interface DeviceAvailability {
  id: string;
  state: "available" | "busy" | "indeterminate" | "current_sonante";
}

export interface VolumeStatus {
  value: number;
  muted: boolean;
  writable: boolean;
  available: boolean;
  backend: "alsa_hardware" | "pipewire" | "mpd_software" | "unavailable";
}

export interface PlaybackDiagnostic {
  code: "output_busy" | "output_unavailable" | "playback_error" | "queue_uncertain" | "persistence_failed" | "format_unsupported";
}

export interface PlaybackOptions {
  repeat: boolean;
  single: "off" | "on" | "oneshot";
  random: boolean;
}
export type PlaybackOptionAction = "cycle_repeat" | "toggle_random";

export type OutputObservation =
  | { state: "unverified"; reason: "unavailable" | "not_playing" | "shared" | "unreadable" | "malformed" | "not_owned" | "ambiguous" | "changed" | "limit" | "busy" }
  | { state: "observed"; configured_endpoint: string; card: number; device: number; subdevice: number; format: string; rate: number; channels: number; kernel_release?: string | null; usb_device?: { vendor_id: string; product_id: string; model: string } | null };

export interface SignalProcessing {
  transport?: { kind: "pcm" | "native_dsd" | "dop"; source_rate: number; output_rate: number; channels: number; epoch: number } | null;
  mpd_volume: number | null;
  crossfade_seconds: number | null;
  mixramp: "off" | "possible" | "unknown";
  replay_gain: { state: "off" | "track" | "album" | "auto" | "unknown" } | { state: "unavailable"; reason: "busy" | "unavailable" | "timeout" | "protocol" | "changed" };
}

export interface EngineEvidence {
  state: "identified";
  evidence_id: "mpd-pcm-eof-20261017" | "mpd-route-20261017" | "mpd-strict-route-20261017";
  scope: "pcm24_96000_stereo_gapless" | "pcm_dsd_route" | "pcm_dsd_route_strict";
  pcm_eof_flush: boolean;
}

export interface PlaybackStatus {
  snapshot_consistent?: boolean;
  processing?: SignalProcessing | null;
  output_observation?: OutputObservation | null;
  engine_evidence?: EngineEvidence | null;
  options: PlaybackOptions | null;
  state: string;
  elapsed: number;
  duration: number;
  audio_format: string;
  current_media: CurrentMedia | null;
  title: string;
  artist: string;
  album: string;
  thumb?: string | null;
  plex_image?: PlexImageRef | null;
  volume: VolumeStatus;
  is_updating: boolean; // <-- Necessário para o ícone de indexação/sincronização
  playback_error?: PlaybackDiagnostic | null;
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
  output_observation?: OutputObservation | null;
  engine_evidence?: EngineEvidence | null;
  health: MpdHealth;
  playback: PlaybackStatus | null;
}

export interface ConfiguredSignalRoute {
  mode: "shared" | "direct";
  endpoint: string | null;
}

export interface QueueArtworkContext {
  blocked: boolean;
  onlineEnabled: boolean;
  generation: number;
}

export interface NowPlayingSnapshot {
  queueArtwork?: QueueArtworkContext;
  sourceInfo?: import("./sourceAudio").PlayerSourceInfo | null;
  route: ConfiguredSignalRoute | null;
  health: MpdHealth;
  playback: PlaybackStatus;
}

export interface AudioLevelFrame {
  leftRms: number;
  rightRms: number;
  leftPeak: number;
  rightPeak: number;
  spectrum: number[];
}

export interface AnalyzerStatus {
  available: boolean;
  reason: string | null;
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

export interface AnalyzerOwner {
  host: "main" | "now-playing";
  sessionId: string;
  generation: number;
  leaseId?: string;
}
export interface AnalyzerEvent<T> { owner: AnalyzerOwner; data: T; }
