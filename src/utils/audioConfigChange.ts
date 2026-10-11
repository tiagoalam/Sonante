import type { AppConfig } from "../types/config";

export function audioRestartExpected(original: AppConfig, next: AppConfig): boolean {
  return original.audio_output_type !== next.audio_output_type
    || original.alsa_device !== next.alsa_device
    || original.dop_enabled !== next.dop_enabled
    || original.audio_buffer_size_kb !== next.audio_buffer_size_kb
    || original.replay_gain !== next.replay_gain;
}

export function localFoldersChanged(original: AppConfig, next: AppConfig): boolean {
  return original.local_folders.length !== next.local_folders.length
    || original.local_folders.some((folder, index) => folder !== next.local_folders[index]);
}

export function localFoldersOnlyChanged(original: AppConfig, next: AppConfig): boolean {
  if (original.first_run || !localFoldersChanged(original, next)) return false;
  const keys = new Set([...Object.keys(original), ...Object.keys(next)]);
  return [...keys].every((key) => key === "local_folders"
    || original[key as keyof AppConfig] === next[key as keyof AppConfig]);
}
