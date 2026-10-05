import type { AppConfig } from "../types/config";

export function audioRestartExpected(original: AppConfig, next: AppConfig): boolean {
  return original.audio_output_type !== next.audio_output_type
    || original.alsa_device !== next.alsa_device
    || original.dop_enabled !== next.dop_enabled
    || original.audio_buffer_size_kb !== next.audio_buffer_size_kb
    || original.replay_gain !== next.replay_gain;
}
