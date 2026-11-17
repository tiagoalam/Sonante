export {AudioSignalDiagram,SignalPath,SignalPathInfo} from "./SignalPath";
import React,{useEffect,useMemo,useState,useSyncExternalStore} from "react";
import {History,Radio,SlidersHorizontal,Layers3,Headphones} from "lucide-react";
import {SmartPlaylists as SharedSmartPlaylists} from "../../src/components/SmartPlaylists";
import {NowPlayingHost as SharedNowPlaying} from "../../src/components/NowPlayingHost";
import {PlayerBar as SharedPlayer} from "../../src/components/PlayerBar";
import {localAlbumCatalog} from "../../src/utils/localAlbumCatalogSession";
import {localArtworkResolver,localAlbumThumbnails} from "../../src/utils/localArtworkSession";
import {artworkReadBlocked} from "../../src/utils/localArtwork";
import {sortLocalAlbums} from "../../src/utils/localAlbumNavigation";
import {LocalBrowserView as SharedLocalBrowserView,type LocalBrowserViewProps} from "../../src/components/LocalBrowserView";
import {localCatalogPresentationAlbums} from "../../src/utils/localAlbumCatalog";
import {useTranslation} from "react-i18next";
import {captureSeek} from "./core";
import {SidebarNavItem as SharedNav} from "../../src/components/SidebarNavItem";
import {SourcesSettings as SharedSources} from "../../src/components/SourcesSettings";
import {SettingsPages as SharedSettings} from "../../src/components/SettingsPages";
export * from "../../src/components/ExperimentalFeatures";
export function SidebarNavItem(props:React.ComponentProps<typeof SharedNav>){
 const{t}=useTranslation();const supported=props.icon===Headphones||props.icon===Layers3||props.icon===History||props.icon===Radio||props.icon===SlidersHorizontal||props.diagnosticAction&&["local","plex","navidrome","search","explore","favorites","playlists","settings","about"].includes(props.diagnosticAction);
 return supported?<SharedNav {...props}/>:<fieldset disabled title={t("windowsPort.pending")} className="min-w-0 opacity-40"><SharedNav {...props}/></fieldset>;
}
export function SettingsPages(props:React.ComponentProps<typeof SharedSettings>){return <SharedSettings {...props}/>}
export function SourcesSettings(props:React.ComponentProps<typeof SharedSources>){return <SharedSources {...props}/>;}
export const SmartPlaylists=SharedSmartPlaylists;
export {localAlbumCatalog};
export type {LocalSubview} from "../../src/components/LocalBrowserView";
export {RemoteControlSetting} from "../../src/components/RemoteControlSetting";
export {UpnpSetting} from "../../src/components/UpnpSetting";


function seekStart(event:React.SyntheticEvent,cursor?:{generation:number;queueIndex:number}){const target=event.target;if(target instanceof HTMLInputElement&&target.type==="range"&&target.step==="0.5")captureSeek(cursor)}
export function PlayerBar(props:React.ComponentProps<typeof SharedPlayer>){return <div className="contents" onPointerDownCapture={event=>seekStart(event)} onKeyDownCapture={event=>{if(!event.repeat&&["ArrowLeft","ArrowRight","ArrowUp","ArrowDown","Home","End","PageUp","PageDown"].includes(event.key))seekStart(event)}}><SharedPlayer {...props}/></div>}
export function NowPlayingHost(props:React.ComponentProps<typeof SharedNowPlaying>){
 const playback=props.snapshot?.playback as (NonNullable<typeof props.snapshot>["playback"] & {windows_generation?:number})|undefined;
 const index=playback?.current_media?.queue_index;const generation=playback?.windows_generation;
 const cursor=typeof generation==="number"&&typeof index==="number"?{generation,queueIndex:index}:undefined;
 return <div className="contents" onPointerDownCapture={event=>seekStart(event,cursor)} onKeyDownCapture={event=>{if(!event.repeat&&["ArrowLeft","ArrowRight","ArrowUp","ArrowDown","Home","End","PageUp","PageDown"].includes(event.key))seekStart(event,cursor)}}><SharedNowPlaying {...props}/></div>;
}
export function LocalWatchNotice(_props:{onOpenSettings:()=>void}){
 useEffect(()=>{let revision="",unavailable=false;
  const changed=(event:Event)=>{const value=(event as CustomEvent).detail;const next=`${value.epoch}:${value.revision}`;if(unavailable||revision&&next!==revision)localAlbumCatalog.requestRefresh();unavailable=false;revision=next;};
  const missing=()=>{unavailable=true};
  window.addEventListener("sonante-windows-library",changed);window.addEventListener("sonante-windows-library-unavailable",missing);
  return()=>{window.removeEventListener("sonante-windows-library",changed);window.removeEventListener("sonante-windows-library-unavailable",missing)}},[]);
 return null;
}
function WindowsLibraryStatus(){
 const{t}=useTranslation();const[status,setStatus]=useState<{epoch?:number;revision?:number;phase?:string;tracks?:number;warnings?:number;metadataWarnings?:number;error?:string|null}>({});const[available,setAvailable]=useState(true);const[dismissed,setDismissed]=useState<string|null>(null);const verifiedScanEpoch=React.useRef<number|null>(null);
 useEffect(()=>{
  const changed=(event:Event)=>{const value=(event as CustomEvent).detail;if(value.phase==="scanning"&&typeof value.epoch==="number")verifiedScanEpoch.current=value.epoch;setStatus(value);setAvailable(true);};
  const missing=()=>{setAvailable(false)};
  window.addEventListener("sonante-windows-library",changed);window.addEventListener("sonante-windows-library-unavailable",missing);
  return()=>{window.removeEventListener("sonante-windows-library",changed);window.removeEventListener("sonante-windows-library-unavailable",missing)}},[]);
 const restoredCache=status.phase==="cached"&&verifiedScanEpoch.current!==status.epoch&&(!status.error||status.error==="windows_library_check_unconfirmed");
 if(!available||restoredCache||status.phase!=="scanning"&&status.phase!=="cached"&&!status.error&&!status.warnings)return null;
 const terminal=status.phase!=="scanning"&&(!!status.error||!!status.warnings);
 const noticeKey=`${status.epoch??"unknown"}:${status.error??"warnings"}:${status.warnings??0}`;
 if(terminal&&dismissed===noticeKey)return null;
 const metadataWarning=status.metadataWarnings??0;const metadataUnconfirmed=metadataWarning>0&&status.error==="windows_library_check_unconfirmed";
 const message=status.phase==="scanning"?t("windowsLibrary.scanning",{count:status.tracks??0}):metadataUnconfirmed&&status.phase!=="cached"?t("windowsLibrary.metadataWarnings",{count:metadataWarning}):status.phase==="cached"?t("windowsLibrary.cached"):status.error?t(`windowsLibrary.errors.${status.error}`,{defaultValue:t(`windowsPort.errors.${status.error}`,{defaultValue:t("windowsPort.failed")})}):t("windowsLibrary.warnings",{count:status.warnings??0});
 const cachedError=status.phase==="cached"&&status.error&&!metadataUnconfirmed?t(`windowsLibrary.errors.${status.error}`,{defaultValue:t(`windowsPort.errors.${status.error}`,{defaultValue:t("windowsPort.failed")})}):null;
 const alert=!!status.error&&!metadataUnconfirmed;
 return <div role={alert?"alert":"status"} aria-live={alert?"assertive":"polite"} aria-atomic="true" className="flex min-w-0 max-w-full items-center justify-end gap-1.5 text-[10px] leading-4"><span className={`min-w-0 truncate ${alert?"text-[color:var(--theme-fg-error,#f87171)]":"text-[color:var(--theme-fg-accent,#E5A00D)]"}`} title={message}>{message}{status.phase==="cached"&&metadataUnconfirmed?` · ${t("windowsLibrary.metadataWarnings",{count:metadataWarning})}`:cachedError?` · ${cachedError}`:""}</span>{terminal&&<button type="button" onClick={()=>setDismissed(noticeKey)} aria-label={t("windowsPort.dismiss")} className="shrink-0 px-1 text-[color:var(--theme-fg-muted,#888)] hover:text-[color:var(--theme-fg-primary,#fff)]">×</button>}</div>
}

const WINDOWS_COVER_PREFETCH_LIMIT=100,WINDOWS_COVER_PREFETCH_CONCURRENCY=1,WINDOWS_COVER_PREFETCH_MAX_SOURCE_CHARS=12*1024*1024;
function decodeCover(data:string):Promise<HTMLImageElement>{return new Promise((resolve,reject)=>{const image=new Image();image.onload=()=>resolve(image);image.onerror=()=>reject(new Error("cover_decode_failed"));image.src=data;if(typeof image.decode==="function")void image.decode().then(()=>resolve(image),reject);});}
function WindowsCoverPrefetch({folders,updating}:{folders:readonly string[];updating:boolean}){
 const catalog=useSyncExternalStore(localAlbumCatalog.subscribe,localAlbumCatalog.getSnapshot);
 const albums=useMemo(()=>catalog.loaded?sortLocalAlbums(localCatalogPresentationAlbums(catalog),"album-asc").slice(0,WINDOWS_COVER_PREFETCH_LIMIT):[],[catalog.albums,catalog.loaded]);
 useEffect(()=>{
  if(!folders.length||!catalog.loaded||artworkReadBlocked(updating,catalog))return;
  const version=catalog.albums;let live=true,index=0;
  const current=()=>{const latest=localAlbumCatalog.getSnapshot();return live&&latest.albums===version&&!artworkReadBlocked(updating,latest)};
  const worker=async()=>{while(live&&index<albums.length){const album=albums[index++];if(localAlbumThumbnails.get(album))continue;
    try{const cover=await localArtworkResolver.resolve(album,false,()=>!current(),undefined,current,true);if(!cover||cover.length>WINDOWS_COVER_PREFETCH_MAX_SOURCE_CHARS||!/^data:image\/(?:jpeg|png|webp);base64,/.test(cover)||!current()||localAlbumThumbnails.get(album))continue;
      const image=await decodeCover(cover);if(current())localAlbumThumbnails.capture(album,image,current,"already_complete");
    }catch{/* Optional local cover prefetch cannot affect catalog or playback. */}
  }};
  void Promise.all(Array.from({length:WINDOWS_COVER_PREFETCH_CONCURRENCY},worker));return()=>{live=false};
 },[albums,catalog.albums,catalog.loaded,catalog.loading,catalog.saveBlocked,catalog.mpdBusy,catalog.error,catalog.manualCheckPlayback,folders,updating]);
 return null;
}
export function LocalBrowserView(props:LocalBrowserViewProps){
 return <SharedLocalBrowserView {...props} hideWindowsCatalogActivity windowsLibraryStatus={<><WindowsLibraryStatus/><WindowsCoverPrefetch folders={props.configuredLocalFolders} updating={props.isLibraryUpdating}/></>}/>;
}

export {AlbumABComparison} from "../../src/components/AlbumABComparison";
