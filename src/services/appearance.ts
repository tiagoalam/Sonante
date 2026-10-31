import {isTauri} from '@tauri-apps/api/core';
import {emit,listen} from '@tauri-apps/api/event';
import {APPEARANCE_KEY,createAppearance,appearanceScheme} from '../utils/appearance';

export const appearance=createAppearance({
 origin:crypto.randomUUID?.()??`${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
 storage:()=>{try{return window.localStorage}catch{return null}},
 apply:value=>{document.documentElement.dataset.theme=value;document.documentElement.style.colorScheme=appearanceScheme(value)},
 onStorage:receive=>{const handler=(e:StorageEvent)=>{if(e.key===APPEARANCE_KEY)receive(e.newValue)};window.addEventListener('storage',handler);return()=>window.removeEventListener('storage',handler)},
 bridge:isTauri()?{listen:async(event,receive)=>listen(event,e=>receive(e.payload)),emit}:undefined,
});
