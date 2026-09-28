export interface AppConfig {
  first_run: boolean;
  alsa_device: string;
  local_folders: string[];
  plex_url: string;
  plex_token: string;
  playback_mode: "http" | "local";
  local_mount_path: string;
  remote_share_path: string;
  dop_enabled: boolean;
  audio_buffer_size_kb: number;
  replay_gain: "off" | "track" | "album";
}
