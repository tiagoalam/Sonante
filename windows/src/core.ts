export * from "../../node_modules/@tauri-apps/api/core.js";
import {invoke as nativeInvoke} from "../../node_modules/@tauri-apps/api/core.js";
import {createWindowsBridge} from "./compat";
import {publishSourceContext} from "./sourceContext";
let telemetry:{generation:number;queueIndex:number}|null=null,seekCursor:{generation:number;queueIndex:number}|null=null;
export function captureSeek(cursor?:{generation:number;queueIndex:number}){const source=cursor??telemetry;seekCursor=source?{...source}:null}
const bridge=createWindowsBridge(nativeInvoke,()=>window.localStorage,value=>window.dispatchEvent(new CustomEvent("sonante-windows-library",{detail:value})),available=>window.dispatchEvent(new CustomEvent("sonante-windows-health",{detail:available})),value=>{telemetry={generation:value.generation,queueIndex:value.queueIndex};publishSourceContext(value.generation,value.engineRunning&&(value.state==="play"||value.state==="pause"))},code=>window.dispatchEvent(new CustomEvent("sonante-windows-error",{detail:code})),()=>window.dispatchEvent(new Event("sonante-windows-library-unavailable")));
const quiet=new Set(["get_local_track_tags","get_local_album_tag_inspection","get_local_album_doctor","get_online_album_metadata_details","search_online_album_metadata","get_scrobbling","set_scrobbling","get_plex_image","windows_smart_playlists","smart_playlist_store","smart_playlist_sources","smart_playlist_catalog","smart_playlist_preview","listening_history","get_local_file_info","navidrome_configuration","get_player_source_info","claim_audio_analyzer_host","release_audio_analyzer_host","start_audio_analyzer","stop_audio_analyzer","get_online_cover_cache_status","get_online_album_cover","windows_configure_sources","windows_check_library"]);
export async function invoke<T>(command:string,args?:Record<string,unknown>):Promise<T>{
 if(command==="seek_playback"&&seekCursor){args={...args,expectedGeneration:seekCursor.generation,expectedIndex:seekCursor.queueIndex};seekCursor=null}
 try{return await bridge<T>(command,args)}catch(error){if(error!=="PLAYBACK_SUPERSEDED"&&!quiet.has(command))window.dispatchEvent(new CustomEvent("sonante-windows-error",{detail:typeof error==="string"?error:"unknown"}));throw error}
}
