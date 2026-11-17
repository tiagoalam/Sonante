import {smartPlaylistsService as original} from "../../src/services/smartPlaylists";
import {invoke} from "./core";
import type {SmartRules} from "../../src/types/smartPlaylist";
export {smartError} from "../../src/services/smartPlaylists";
// Reuse the full original catalog expansion, genre family classifier, local read
// scheduling, cancellation, round-robin and identity handling. A final native
// fence rejects a catalog/root change during that asynchronous expansion.
export const smartPlaylistsService={...original,preview:async(rules:SmartRules,current:()=>boolean=()=>true,random:()=>number=Math.random)=>{
 if(!current())throw "SMART_PLAYLIST_CANCELLED";
 const token=await invoke<string>("windows_smart_playlists",{operation:"epoch",args:{source:rules.source}});
 if(!current())throw "SMART_PLAYLIST_CANCELLED";
 const value=await original.preview(rules,current,random);
 if(!current())throw "SMART_PLAYLIST_CANCELLED";
 await invoke("windows_smart_playlists",{operation:"epoch",args:{source:rules.source,expected:token}});
 if(!current())throw "SMART_PLAYLIST_CANCELLED";
 return value;
}};
