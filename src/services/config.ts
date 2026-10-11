import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, ConfigSaveOperation, ConfigSaveResponse, QueueClearAuthorization } from "../types/config";
import type { LocalImportPreview } from "../types/localImportPreview";
import type { AudioDevice, DeviceAvailability } from "../types/audio";

export const configService = {
  getConfig: (): Promise<AppConfig> => invoke<AppConfig>("get_config"),
  getAudioDevices: (): Promise<AudioDevice[]> => invoke<AudioDevice[]>("get_audio_devices"),
  checkDirectDeviceAvailability: (deviceId: string): Promise<DeviceAvailability[]> =>
    invoke<DeviceAvailability[]>("check_direct_device_availability", { deviceId }),
  saveConfig: (newConfig: AppConfig, queueClearAuthorization?: QueueClearAuthorization): Promise<ConfigSaveResponse> =>
    invoke<ConfigSaveResponse>("save_config", {
      newConfig,
      ...(queueClearAuthorization ? { queueClearAuthorization } : {}),
    }),
  getLocalImportPreview: (operationId: string): Promise<LocalImportPreview> =>
    invoke<LocalImportPreview>("get_local_import_preview", { operationId }),
  getSaveOperation: (): Promise<ConfigSaveOperation | null> =>
    invoke<ConfigSaveOperation | null>("get_config_save_operation"),
};
