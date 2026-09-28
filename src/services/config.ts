import { invoke } from "@tauri-apps/api/core";
import { AppConfig } from "../types/config";
import { AudioDevice } from "../types/audio";

export const configService = {
  getConfig: (): Promise<AppConfig> => invoke<AppConfig>("get_config"),
  getAudioDevices: (): Promise<AudioDevice[]> => invoke<AudioDevice[]>("get_audio_devices"),
  saveConfig: (newConfig: AppConfig): Promise<void> =>
    invoke<void>("save_config", { newConfig }),
};
