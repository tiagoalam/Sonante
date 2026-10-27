import i18n from "i18next";
import { hasDirectVolumeChoice, confirmDirectVolumeConfig } from "../utils/directVolumeChoice.ts";
import { playbackIntentToken, type PlaybackIntent } from "../utils/playbackIntent.ts";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, ConfigSaveOperation, ConfigSaveResponse, QueueClearAuthorization } from "../types/config";
import type { LocalImportPreview } from "../types/localImportPreview";
import type { AudioDevice, DeviceAvailability } from "../types/audio";

function askDirectVolumeChoice():boolean{return window.confirm(i18n.t('player.directVolumeChoiceConfirm'));}
export async function ensureDirectOutputChoice(intent:PlaybackIntent):Promise<void>{
 const token=await playbackIntentToken(intent),config=await invoke<AppConfig>('get_config');
 if(!intent.current())throw 'PLAYBACK_SUPERSEDED';if(hasDirectVolumeChoice(config))return;
 try{await confirmDirectVolumeConfig(config,askDirectVolumeChoice);}catch(error){if(error==='DIRECT_CHOICE_CANCELLED'){intent.cancel();throw 'PLAYBACK_SUPERSEDED';}throw error;}
 if(!intent.current())throw 'PLAYBACK_SUPERSEDED';
 await invoke('confirm_direct_volume_choice',{expectedConfig:config,intent:token});
 if(!await intent.confirmCurrent())throw 'PLAYBACK_SUPERSEDED';
}
export const configService = {
  getConfig: (): Promise<AppConfig> => invoke<AppConfig>("get_config"),
  getAudioDevices: (): Promise<AudioDevice[]> => invoke<AudioDevice[]>("get_audio_devices"),
  checkDirectDeviceAvailability: (deviceId: string): Promise<DeviceAvailability[]> =>
    invoke<DeviceAvailability[]>("check_direct_device_availability", { deviceId }),
  saveConfig: async (newConfig: AppConfig, queueClearAuthorization?: QueueClearAuthorization): Promise<ConfigSaveResponse> =>
    invoke<ConfigSaveResponse>("save_config", {
      newConfig: await confirmDirectVolumeConfig(newConfig,askDirectVolumeChoice),
      ...(queueClearAuthorization ? { queueClearAuthorization } : {}),
    }),
  getLocalImportPreview: (operationId: string): Promise<LocalImportPreview> =>
    invoke<LocalImportPreview>("get_local_import_preview", { operationId }),
  getLocalImportArtwork: (operationId: string, reference: string): Promise<string | null> =>
    invoke<string | null>("get_local_import_artwork", { operationId, reference }),
  getSaveOperation: (): Promise<ConfigSaveOperation | null> =>
    invoke<ConfigSaveOperation | null>("get_config_save_operation"),
};
