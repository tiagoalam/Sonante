import type { AppConfig } from '../types/config';
export const directVolumeContext=(c:AppConfig)=>c.audio_output_type==='pipewire'||c.alsa_device==='default'?null:{version:1,endpoint:c.alsa_device,dop_enabled:c.dop_enabled};
export function hasDirectVolumeChoice(c:AppConfig){const x=directVolumeContext(c),p=c.direct_volume_choice;return !x||!!p&&p.version===x.version&&p.endpoint===x.endpoint&&p.dop_enabled===x.dop_enabled;}
export async function confirmDirectVolumeConfig(c:AppConfig,ask:()=>boolean|Promise<boolean>):Promise<AppConfig>{const x=directVolumeContext(c);if(!x)return c.direct_volume_choice?{...c,direct_volume_choice:null}:c;if(hasDirectVolumeChoice(c))return c;if(!await ask())throw 'DIRECT_CHOICE_CANCELLED';return{...c,direct_volume_choice:x};}
