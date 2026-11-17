import type {AppConfig} from "../../src/types/config";
import type {LocalAlbum,LocalAlbumDiscTracks} from "../../src/types/local";
import type {PlaybackStatus,TrackMetadata,PlaybackOptions,AudioRouteProjection} from "../../src/types/audio";
import type {FavoriteAlbum} from "../../src/types/favorite";
import type {DspSnapshot} from "../../src/types/equalizer";
type RemoteSource={kind:"navidrome";account_id:string;song_id:string}|{kind:"plex";server_id:string;part_key:string;rating_key:string};
type NativeTrack={plexImage?:{server_id:string;path:string}|null;remoteSource?:RemoteSource|null;remoteConnectionId?:string|null;id:string;title:string;artist:string;album:string;disc:number;fileName:string;catalogAlbumId?:string|null;upnp?:{sessionId:string;artwork:string|null}|null;radio?:{stationId:string;thumb:string|null}|null};
type AudioSettings=Pick<AppConfig,"audio_output_type"|"alsa_device"|"dop_enabled"|"audio_buffer_size_kb"|"replay_gain"|"direct_volume_choice"|"direct_muted">;
const audioSettings=(c:AppConfig):AudioSettings=>({audio_output_type:c.audio_output_type,alsa_device:c.alsa_device,dop_enabled:c.dop_enabled,audio_buffer_size_kb:c.audio_buffer_size_kb,replay_gain:c.replay_gain,direct_volume_choice:c.direct_volume_choice??null,direct_muted:c.direct_muted??false});
type NativeStatus={equalizer?:DspSnapshot;audioRoute?:AudioRouteProjection;audioSettings?:AudioSettings;direct?:{token:string|null;gain:"pending"|"unity"|"muted"|"changed"|null;muted:boolean;available:boolean;consistent:boolean}|null;logicalState?:string;observation?:string;generation:number;engineRunning:boolean;state:string;elapsed:number;duration:number;volume:number;audio:string;warning:string|null;options?:PlaybackOptions|null;queueRevision?:string;queue:NativeTrack[];queueDurations?:(number|null)[];queueIndex:number};
type NativeInvoke=<T=unknown>(name:string,args?:Record<string,unknown>)=>Promise<T>;
type Store=Pick<Storage,"getItem"|"setItem">;
type PlexConfig=AppConfig&{windows_plex_saved?:boolean;windows_plex_revision?:string|null};
export const trackUri=(track:NativeTrack)=>track.upnp?`upnp/${track.upnp.sessionId}`:track.radio?`radio/${track.radio.stationId}`:track.catalogAlbumId?`windows/${track.catalogAlbumId}/${track.disc}/${track.id}`:`windows-file/${track.id}`;
export function queueMetadata(track:NativeTrack,duration?:unknown):TrackMetadata{
 if(track.upnp)return{title:track.title,artist:track.artist,album:track.album,thumb:track.upnp.artwork,media_locator:{kind:"upnp",session_id:track.upnp.sessionId}};
 if(track.radio)return{title:track.artist,artist:track.artist,album:"",thumb:track.radio.thumb,media_locator:{kind:"radio",station_id:track.radio.stationId}};
 const source=track.remoteSource;
 const locator=source?.kind==="navidrome"?{kind:"navidrome" as const,account_id:source.account_id,song_id:source.song_id,connection_id:track.remoteConnectionId??""}:source?.kind==="plex"?{kind:"plex" as const,server_id:source.server_id,part_key:source.part_key,rating_key:source.rating_key}:{kind:"local" as const,uri:trackUri(track)};
 return{title:track.title,artist:track.artist,album:track.album,...(typeof duration==="number"&&Number.isFinite(duration)&&duration>0?{duration}:{}),media_locator:locator,...(source?.kind==="plex"&&track.plexImage?.server_id===source.server_id?{plex_image:track.plexImage}:{})};
}
function queueProjection(p:NativeStatus):TrackMetadata[]{
 const durations=Array.isArray(p.queueDurations)&&p.queueDurations.length===p.queue.length?p.queueDurations:null;
 return p.queue.map((track,index)=>queueMetadata(track,durations?.[index]));
}
export function playbackProjection(p:NativeStatus,updating:boolean):PlaybackStatus{
 const track=p.queue[p.queueIndex],source=track?.remoteSource;
 const current=track?(track.upnp?{kind:"upnp" as const,session_id:track.upnp.sessionId,queue_index:p.queueIndex}:track.radio?{kind:"radio" as const,station_id:track.radio.stationId,queue_index:p.queueIndex}:source?.kind==="navidrome"?(track.remoteConnectionId?{kind:"navidrome" as const,connection_id:track.remoteConnectionId,song_id:source.song_id,queue_index:p.queueIndex}:null):source?.kind==="plex"?{kind:"plex" as const,server_id:source.server_id,part_key:source.part_key,queue_index:p.queueIndex}:{kind:"local" as const,uri:trackUri(track),queue_index:p.queueIndex}):null;
 const direct=p.audioSettings?.audio_output_type==="alsa"&&p.audioSettings.alsa_device!=="default";
 return{windows_mode:direct?"direct":"shared",windows_generation:p.generation,snapshot_consistent:p.direct?.consistent??true,state:!p.engineRunning&&track?(p.logicalState??"pause"):p.state==="idle"?"stop":p.state,elapsed:p.elapsed,duration:p.duration,audio_format:p.audio,current_media:current,...(track?.upnp?{thumb:track.upnp.artwork}:track?.radio?{thumb:track.radio.thumb}:{}),...(source?.kind==="plex"&&track?.plexImage?.server_id===source.server_id?{plex_image:track.plexImage}:{}),title:track?.title??"",artist:track?.artist??"",album:track?.album??"",volume:direct?{value:p.volume,muted:p.direct?.muted??false,writable:false,available:p.direct?.available??false,backend:"direct_equipment",direct_token:p.direct?.token??undefined,direct_gain:p.direct?.gain??"pending"}:{value:p.volume,muted:p.volume===0,writable:!!track,available:!!track,backend:track?"mpd_software":"unavailable",shared_token:`windows:${p.generation}`},is_updating:updating,options:p.options??null,playback_error:p.warning?{code:"persistence_failed"}:null,processing:null,output_observation:{state:"unverified",reason:direct?"unavailable":"shared"},engine_evidence:null,shared_dsp:p.equalizer??null,audio_route:p.audioRoute??null} as PlaybackStatus & {windows_generation:number};
}
function usageMode(value:unknown):"full"|"simple"{
 if(value===undefined)return "full";
 if(value==="full"||value==="simple")return value;
 throw "invalid_argument";
}
export function createWindowsBridge(native:NativeInvoke,storage:()=>Store,observeLibrary:(value:Record<string,unknown>)=>void=()=>{},observeHealth:(available:boolean)=>void=()=>{},observeStatus:(value:NativeStatus)=>void=()=>{},reportWarning:(code:string)=>void=()=>{},observeLibraryUnavailable:()=>void=()=>{}){
 let latest:NativeStatus|null=null;let volumeRevision=0;let sampleSequence=0,publishedSample=0,libraryUnavailable=false;
 let observedSerial:bigint|null=null;
 const status=async()=>{
  const value=await native<NativeStatus>("windows_status");const raw=value.observation;
  // Older hosts/fixtures have no serial. Never invent one, or accept a missing
  // serial after this bridge has observed the native sequencing contract.
  if(raw===undefined){if(observedSerial!==null)throw "windows_status_unconfirmed";observeStatus(value)}
  else{
   if(typeof raw!=="string"||! /^[1-9][0-9]{0,19}$/.test(raw))throw "windows_status_unconfirmed";
   const serial=BigInt(raw);if(serial>18446744073709551615n)throw "windows_status_unconfirmed";
   if(observedSerial===null||serial>observedSerial){observedSerial=serial;observeStatus(value)}
  }
  // Keep each caller's actual response; only publication of shared telemetry is
  // fenced. A stale caller is not handed a newer cached/fabricated snapshot.
  latest=value;return value;
 };
 const action=async(kind:Record<string,unknown>,intent?:unknown)=>{const p=await status();latest=await native<NativeStatus>("windows_action",{generation:p.generation,action:kind,intent:typeof intent==="string"?intent:null});return latest;};
 // Native storage is authoritative after the first atomic import. Keep the
 // legacy WebView value untouched as a rollback backup, never a live fallback.
 const favoriteRows=async():Promise<FavoriteAlbum[]>=>{
  const current=await native<FavoriteAlbum[]|null>("windows_favorites",{operation:"list"});
  if(current!==null)return current;
  const legacy=storage().getItem("sonante_windows_favorites")??"[]";
  if(legacy.length>4*1024*1024)throw "windows_favorites_invalid";
  return native<FavoriteAlbum[]>("windows_favorites",{operation:"import",legacy});
 };
 const artworkProgress=async():Promise<Record<string,unknown>>=>{
  const current=await native<Record<string,unknown>|null>("windows_artwork_progress",{operation:"list",args:{}});
  if(current!==null)return current;
  const legacy=storage().getItem("sonante_windows_artwork")??"{}";
  if(legacy.length>4*1024*1024)throw "windows_artwork_progress_invalid";
  return native<Record<string,unknown>>("windows_artwork_progress",{operation:"import",args:{legacy}});
 };
 const config=async():Promise<AppConfig>=>{const prefs=await native<{usage_mode?:"full"|"simple";first_run?:boolean;experimental_features?:boolean;online_artwork_enabled?:boolean;roots:string[];sources:{local:boolean;navidrome:boolean;plex:boolean};plex:{url:string;server_id:string|null;server_name:string|null;saved:boolean;revision:string|null};audio?:AudioSettings}>("windows_preferences");return{usage_mode:usageMode(prefs.usage_mode),windows_plex_saved:prefs.plex.saved,windows_plex_revision:prefs.plex.revision,first_run:prefs.first_run===true,sources:prefs.sources,experimental_features:prefs.experimental_features===true,alsa_device:"default",audio_output_type:"pipewire",local_folders:prefs.roots,online_artwork_enabled:prefs.online_artwork_enabled===true,plex_token:"",plex_url:prefs.plex.url,plex_server_id:prefs.plex.server_id,plex_server_name:prefs.plex.server_name,playback_mode:"http",local_mount_path:"",remote_share_path:"",dop_enabled:false,audio_buffer_size_kb:4096,replay_gain:"off",...(prefs.audio??{})} as PlexConfig};
 const cover=async(path:unknown)=>{if(typeof path!=="string")throw "invalid_argument";const parts=path.split('/');if(parts[0]!=="windows"||!parts[1])return null;return native<string|null>("windows_library_cover",{albumId:parts[1]})};
 return async function invoke<T=unknown>(command:string,args:Record<string,unknown>={}):Promise<T>{
  let result:unknown;
  if(command.startsWith("windows_")||command.startsWith("plugin:")||command.startsWith("navidrome_")||["history_plex_navigation", "plex_create_pin", "plex_check_pin", "plex_get_servers", "get_plex_libraries", "get_plex_collections", "get_collection_albums", "get_artist_portrait", "get_artist_albums", "get_artist_top_tracks", "search_plex", "get_album_tracks", "get_plex_discover", "get_plex_discover_recent", "get_plex_discover_children", "get_plex_discover_artist", "get_plex_library_mix_years", "get_plex_library_mix_styles", "get_plex_library_mix", "get_plex_albums", "get_plex_image", "open_external_url", "resolve_player_plex_navigation"].includes(command))return native<T>(command,args);
  switch(command){
   case "get_local_diagnostics":case "get_online_album_cover":case "get_online_cover_cache_status":case "get_edition_profile":case "get_edition_cache_namespace":case "get_local_track_tags":case "get_local_album_tag_inspection":case "get_local_album_doctor":case "get_online_album_metadata_details":case "search_online_album_metadata":result=await native(command,args);break;
   case "ab_tracks":case "ab_begin":case "ab_load":case "ab_decode":case "ab_start":case "ab_alive":case "ab_end":case "ab_cancel":result=await native(command,args);break;
   case "get_upnp_receiver":case "set_upnp_receiver":result=await native(command,args);break;
   case "get_remote_control":case "set_remote_control":case "pair_remote_control":case "revoke_remote_control":case "remote_control_qr":result=await native(command,args);break;
   case "get_scrobbling":case "set_scrobbling":result=await native(command,args);break;
   case "get_equalizer":case "edit_equalizer":case "pick_equalizer_preset":case "export_equalizer_preset":case "apply_equalizer":case "compare_equalizer":result=await native(command,args);break;
   case "search_radio":case "radio_filters":case "radio_favorites":case "play_radio":case "stop_radio":result=await native(command,args);break;
   case "get_local_file_info":result=await native("windows_inspect_file",{path:args.path});break;
   case "claim_audio_analyzer_host":case "release_audio_analyzer_host":case "start_audio_analyzer":case "stop_audio_analyzer":result=await native(command,args);break;
   case "smart_playlist_store":case "smart_playlist_sources":case "smart_playlist_catalog":case "smart_playlist_preview": {
    if(command==="smart_playlist_preview"&&(args.rules as {favorites_only?:boolean;source?:{kind:string}})?.favorites_only&&(args.rules as {source?:{kind:string}})?.source?.kind!=="navidrome")await favoriteRows();
    result=await native("windows_smart_playlists",{operation:command.slice("smart_playlist_".length),args});break;
   }
   case "listening_history":result=await native("windows_listening_history",{enabled:args.enabled??null,clear:args.clear??false});break;
   case "get_sleep_timer":result=await native("windows_get_sleep_timer");break;
   case "set_sleep_timer":result=await native("windows_set_sleep_timer",{minutes:args.minutes??null});break;
   case "list_playlists":case "create_playlist":case "create_playlist_with_items":case "rename_playlist":case "delete_playlist":case "add_playlist_item":case "add_playlist_items":case "remove_playlist_item":case "reorder_playlist_items":
    result=await native("windows_playlist_store",{operation:command,args});break;
   case "resolve_playlist_items":result=await native("windows_playlist_resolve",{playlistId:args.playlistId});break;
   case "play_playlist":result=await native("windows_playlist_play",{playlistId:args.playlistId,startItemId:args.startItemId??null,shuffle:args.shuffle??false,intent:args.intent});break;
   case "get_config": result=await config();break;
   case "get_audio_devices":result=await native("windows_audio_devices");break;
   case "check_direct_device_availability":result=[{id:args.deviceId,state:"indeterminate"}];break;
   case "confirm_direct_volume_choice":result=await native("windows_confirm_direct",{expected:audioSettings(args.expectedConfig as AppConfig),intent:args.intent});break;
   case "set_direct_mute":result=await native("windows_direct_mute",{muted:args.muted,token:args.token});break;
   case "get_config_save_operation":result=await native("windows_config_save_operation");break;
   case "get_local_import_preview":result=await native("windows_local_import_preview",{operationId:args.operationId});break;
   case "get_local_import_artwork":result=await native("windows_local_import_artwork",{operationId:args.operationId,reference:args.reference});break;
   case "save_config":{
    const c=args.newConfig as AppConfig;
    const mode=usageMode(c?.usage_mode);
    if(!c||!["pipewire","alsa"].includes(c.audio_output_type)||c.dop_enabled||!["off","track","album"].includes(c.replay_gain))throw "windows_feature_unavailable";
    if(c.online_artwork_enabled!==undefined&&typeof c.online_artwork_enabled!=="boolean")throw "invalid_argument";
    if(c.experimental_features!==undefined&&typeof c.experimental_features!=="boolean")throw "invalid_argument";
    if(!Array.isArray(c.local_folders)||c.local_folders.length>32||c.local_folders.some(root=>typeof root!=="string"))throw "windows_folder_limit";
    const previous=await config();const sources={local:c.sources?.local??true,navidrome:c.sources?.navidrome??false,plex:c.sources?.plex??false};
    if(typeof sources.local!=="boolean"||typeof sources.navidrome!=="boolean"||typeof sources.plex!=="boolean"||!sources.local&&!sources.navidrome&&!sources.plex)throw "windows_source_required";
    const folders=JSON.stringify(previous.local_folders)!==JSON.stringify(c.local_folders);
    const changed=c.plex_token!==""||previous.plex_url!==c.plex_url||previous.plex_server_id!==c.plex_server_id||previous.plex_server_name!==c.plex_server_name||(previous as PlexConfig).windows_plex_saved!==(c as PlexConfig).windows_plex_saved;
    const plexAccount=changed?{url:c.plex_url,server_id:c.plex_server_id??null,server_name:c.plex_server_name??null,credential:c.plex_token?{kind:"new",token:c.plex_token}:(c as PlexConfig).windows_plex_saved?{kind:"saved"}:{kind:"clear"}}:null;
    const audioChanged=JSON.stringify(audioSettings(previous))!==JSON.stringify(audioSettings(c));
    let response:unknown={status:"completed"};
    if(previous.usage_mode!==mode||previous.online_artwork_enabled!==(c.online_artwork_enabled===true)||previous.experimental_features!==(c.experimental_features===true)||previous.first_run&&!c.first_run||folders||changed||audioChanged||previous.sources?.local!==sources.local||previous.sources?.navidrome!==sources.navidrome||previous.sources?.plex!==sources.plex)response=await native("windows_configure_sources",{roots:folders?c.local_folders:null,sources,plexAccount,usageMode:mode,experimentalFeatures:c.experimental_features===true,onlineArtworkEnabled:c.online_artwork_enabled===true,...(previous.first_run&&!c.first_run?{completeSetup:true}:{}),...(audioChanged?{audio:audioSettings(c)}:{})});
    const indexing=sources.local&&(folders||!previous.sources?.local);
    if(response===null||response===undefined){if(indexing)throw "windows_config_uncertain";response={status:"completed"};}
    const ack=response as {status?:unknown;operation?:{id?:unknown;phase?:unknown}};
    if(ack.status!=="completed"&&(ack.status!=="accepted"||typeof ack.operation?.id!=="string"||! /^[1-9][0-9]{0,19}$/.test(ack.operation.id)||BigInt(ack.operation.id)>18446744073709551615n||!["preparing","indexing","validating","completed","failed","uncertain"].includes(String(ack.operation.phase))))throw "windows_config_uncertain";
    result=response;break;
   }
   case "pick_directory":result=await native("windows_pick_directory");break;
   case "get_mpd_status_snapshot":{
    const sample=++sampleSequence;
    const [playback,library]=await Promise.allSettled([status(),native<Record<string,unknown>>("windows_library_status")]);
    // Both requests retain their original cadence and await. Each source owns
    // only its observation; a failed catalog read is not an MPD health failure.
    // Late pairs return their own result, never a fabricated newer cached one,
    // but cannot overwrite newer published availability/catalog observations.
    if(sample>publishedSample){
     publishedSample=sample;
     if(library.status==="fulfilled"){libraryUnavailable=false;observeLibrary(library.value)}
     else{observeLibraryUnavailable();if(!libraryUnavailable)reportWarning("windows_library_status_unavailable");libraryUnavailable=true}
     if(playback.status==="fulfilled")observeHealth(true);
     else if(playback.reason==="engine_busy")reportWarning(playback.reason);else observeHealth(false);
    }
    if(playback.status==="fulfilled"){
     const projected=playbackProjection(playback.value,false);
     result={health:{state:"available"},audio_route:projected.audio_route??null,playback:projected};
    }else result={health:{state:"unavailable",reason:"protocol_unavailable"},audio_route:null,playback:null};break;
   }
   case "get_playback_status":result=playbackProjection(await status(),false);break;
   case "get_queue":result=queueProjection(await status());break;
   case "get_queue_edit_snapshot":{const p=await status();result={tracks:queueProjection(p),revision:p.queueRevision??String(p.generation)};break;}
   case "enqueue_tracks":{
    const tracks=args.tracks as TrackMetadata[];
    if(!Array.isArray(tracks)||tracks.length===0||tracks.length>1000||!["next","end"].includes(String(args.placement)))throw "invalid_argument";
    if(tracks.some(t=>t.media_locator?.kind==="navidrome"||t.media_locator?.kind==="plex"))result=await native("windows_enqueue_tracks",{tracks,placement:args.placement,intent:args.intent});
    else{const uris=tracks.map(t=>t.media_locator?.kind==="local"?t.media_locator.uri:null);if(uris.some(u=>typeof u!=="string"))throw "invalid_argument";result=await native("windows_enqueue",{uris,placement:args.placement,intent:args.intent});}break;
   }
   case "edit_queue":result=await native("windows_edit_queue",{revision:args.revision,action:args.action,intent:args.intent});break;
   case "admit_playback_intent":result=await native("windows_intent",{operation:"admit",token:null});break;
   case "playback_intent_current":result=await native("windows_intent",{operation:"current",token:args.token});break;
   case "cancel_playback_intent":result=await native("windows_intent",{operation:"cancel",token:args.token});break;
   case "play_tracks":case "play_uris":{
    if(command==="play_tracks"&&Array.isArray(args.tracks)&&(args.tracks as TrackMetadata[]).some(t=>t.media_locator?.kind==="navidrome"||t.media_locator?.kind==="plex")){result=await native("windows_play_tracks",{tracks:args.tracks,startIndex:args.startIndex,intent:args.intent});break;}
    const uris=command==="play_uris"?args.uris:(args.tracks as TrackMetadata[])?.map(t=>t.media_locator?.kind==="local"?t.media_locator.uri:t.uri);
    if(!Array.isArray(uris)||uris.some(u=>typeof u!=="string"))throw "invalid_argument";
    latest=await native<NativeStatus>("windows_play_selection",{uris,startIndex:args.startIndex,intent:args.intent});break;
   }
   case "toggle_playback":await action({kind:"set_playing",want_play:args.wantPlay},args.intent);break;
   case "change_playback_option":{const p=await action({kind:"change_options",action:args.action,expected:args.expected});if(!p.options)throw "playback_options_changed";result=p.options;break;}
   case "next_track":await action({kind:"next"},args.intent);break;
   case "previous_track":await action({kind:"previous"},args.intent);break;
   case "play_queue_index":await action({kind:"jump",index:args.index},args.intent);break;
   case "clear_queue":await action({kind:"release"},args.intent);break;
   case "seek_playback":{const p=await status();latest=await native<NativeStatus>("windows_action",{generation:args.expectedGeneration??p.generation,action:{kind:"seek",seconds:args.seconds,index:args.expectedIndex??p.queueIndex},intent:null});break;}
   case "set_volume":{const revision=++volumeRevision;const value=await native<NativeStatus>("windows_gain",{action:{kind:"volume",value:args.volume}});if(revision!==volumeRevision)throw "PLAYBACK_SUPERSEDED";latest=value;break;}
   case "set_shared_mute":{
    if(typeof args.muted!=="boolean"||typeof args.token!=="string")throw "invalid_argument";
    const revision=++volumeRevision;
    const value=await native<NativeStatus>("windows_gain",{action:{kind:"mute",muted:args.muted,token:args.token}});
    if(revision!==volumeRevision)throw "PLAYBACK_SUPERSEDED";
    latest=value;break;
   }
   case "list_local_directory":result=await native("windows_directory",{path:args.path});break;
   case "get_local_albums":result=(await native<{albums:LocalAlbum[]}>("windows_catalog")).albums;break;
   case "get_local_album_tracks":result=await native<LocalAlbumDiscTracks[]>("windows_catalog_tracks",{albumId:args.albumId});break;
   case "search_local_tracks":result=await native("windows_search",{query:args.query});break;
   case "get_local_cover":result=await cover(args.path);break;
   case "get_local_covers":{
    if(!Array.isArray(args.paths)||args.paths.length>2)throw "invalid_argument";
    result=await Promise.all(args.paths.map(async path=>{try{return{Ok:await cover(path)}}catch{return{Err:"cover_unavailable"}}}));break;
   }
   case "resolve_local_library_path":{if(typeof args.path!=="string"||!args.path.startsWith("windows/"))throw "invalid_argument";result=args.path;break;}
   case "check_local_library":result=await native<boolean>("windows_check_library",{sourceId:args.sourceId??null,expectedFolders:args.expectedFolders??null});break;
   case "rescan_library":await native("windows_rescan_library");break;
   case "get_favorites":{
    const list=await favoriteRows();
    if(!Array.isArray(list)||list.length>10000)throw "windows_preferences_unavailable";
    const hydrated:FavoriteAlbum[]=Array(list.length);let cursor=0,failed=false;
    const worker=async()=>{while(cursor<list.length){const index=cursor++,album=list[index];
     if(album?.source==="plex") {const row=album as FavoriteAlbum&{plex_thumb?:FavoriteAlbum["plex_image"]};hydrated[index]={...album,thumb:null,plex_image:row.plex_thumb??null};continue;}
     if(!album||typeof album.id!=="string"||album.source!=="local"||typeof album.path_or_key!=="string")throw "windows_preferences_unavailable";
     let thumb:string|null=null,exists:boolean|undefined;
     try{const info=await native<{exists:boolean;thumb:string|null}>("windows_favorite_cover",{albumId:album.path_or_key.split('/')[1]});thumb=info.thumb;exists=info.exists}catch{failed=true}
     hydrated[index]={...album,exists,thumb,plex_image:null};
    }};
    await Promise.all([worker(),worker()]);if(failed)reportWarning("favorite_cover_unavailable");result=hydrated;break;
   }
   case "toggle_favorite":{
    const album=args.album as FavoriteAlbum;if(!["local","plex"].includes(album?.source)||typeof album.id!=="string"||album.source==="local"&&!album.path_or_key?.startsWith("windows/"))throw "invalid_argument";
    await favoriteRows();
    result=await native<boolean>("windows_favorites",{operation:"toggle",album:{...album,thumb:null,plex_image:album.source==="plex"?album.plex_image:null}});break;
   }
   case "get_artwork_enrichment_progress":result=await artworkProgress();break;
   case "save_artwork_enrichment_progress":{
    if(!Array.isArray(args.changes)||args.changes.length>256)throw "invalid_argument";
    await artworkProgress();result=await native("windows_artwork_progress",{operation:"save",args:{changes:args.changes}});break;
   }
   case "get_player_source_info":result=await native("windows_player_source_info",{media:args.media});break;
   case "set_window_title":result=await native("windows_set_title",{title:args.title});break;
   default: throw "windows_feature_unavailable";
  }
  return result as T;
 };
}
