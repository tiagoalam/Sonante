import {useEffect,useMemo,useState,useSyncExternalStore} from "react";
import {useTranslation} from "react-i18next";
import {invoke} from "../../node_modules/@tauri-apps/api/core.js";
import {getSourceContext,subscribeSourceContext} from "./sourceContext";
import {requestObservation,type OutputResult} from "./outputObservationModel";
import type {PlaybackStatus,MpdHealth} from "../../src/types/audio";
export function OutputObservation({status,health}:{status:PlaybackStatus|null;health?:MpdHealth|null}){
 const{t,i18n}=useTranslation();const native=useSyncExternalStore(subscribeSourceContext,getSourceContext);
 const[revision,setRevision]=useState(0);const[result,setResult]=useState<{context:object;result:OutputResult}|null>(null);
 const media=status?.current_media;const live=native.live&&status?.state==="play"&&health?.state==="available";
 const scope=JSON.stringify([native.generation,live,media,revision]);
 // Object identity fences an ABA transition (play → pause → play) even when
 // its URI, generation and cursor return to exactly the same values.
 const context=useMemo(()=>({generation:native.generation,live,scope}),[scope,native.generation,live]);
 const shown=result?.context===context?result.result:null;
 useEffect(()=>{
  if(!context.live)return;
  return requestObservation(()=>invoke("windows_observe_output",{generation:context.generation}),result=>setResult({context,result}));
 },[context]);
 const observed=shown?.kind==="observed"?shown.value:null;
 return <section className="space-y-2 border-t border-[color:var(--theme-border-soft,#333)] pt-3" aria-label={t("windowsPort.outputObservationTitle")}>
  <h3 className="font-semibold">{t("windowsPort.outputObservationTitle")}</h3>
  {observed?<><p className="break-words">{observed.deviceName}</p><p>{t("windowsPort.outputObservedAt",{time:new Date(observed.observedAt).toLocaleTimeString(i18n.language)})}</p></>:<p role="status">{live&&!shown?t("windowsPort.outputChecking"):t(live?"windowsPort.outputUnavailable":"windowsPort.outputNotPlaying")}{shown?.kind==="unavailable"&&["busy","timeout","ambiguous"].includes(shown.reason)?` · ${t(`windowsPort.outputReason_${shown.reason}`)}`:""}</p>}
  {observed?.endpointVolume&&<><p>{t("windowsPort.outputEndpointVolume",{value:observed.endpointVolume.value})}{observed.endpointVolume.muted?` · ${t("windowsPort.outputEndpointMuted")}`:""}</p><p>{t("windowsPort.outputEndpointReadOnly")}</p></>}
  <p>{t("windowsPort.outputObservationScope")}</p>
  <button type="button" disabled={!live||!shown} onClick={()=>setRevision(v=>v+1)} className="rounded border border-[color:var(--theme-border-soft,#444)] px-3 py-1.5 disabled:opacity-40">{t("windowsPort.outputRefresh")}</button>
 </section>;
}
