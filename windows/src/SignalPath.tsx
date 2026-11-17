//! OS-specific route facts inside the original shared signal dialog.
import React from "react";
import {OutputObservation} from "./OutputObservation";
import {useTranslation} from "react-i18next";
import {SignalPath as LinuxSignalPath,SignalPathInfo} from "../../src/components/SignalPath";
export {SignalPathInfo};
import {signalPathInformation} from "../../src/utils/signalPath";
export function AudioSignalDiagram({mode="shared",equalized=false}:{mode?:"shared"|"direct";device?:string;equalized?:boolean}){const{t}=useTranslation();const route=mode==="direct"?"MPD → WASAPI Exclusive → Windows":equalized?"MPD → CamillaDSP → WASAPI Shared → Windows":"MPD → WASAPI Shared → Windows";return <div className="rounded-xl border border-[color:var(--theme-border-soft,#333)] p-4 text-xs text-[color:var(--theme-fg-secondary,#aaa)]"><strong className="block mb-2">{route}</strong>{t(mode==="direct"?"windowsPort.directScope":equalized?"windowsPort.eqScope":"windowsPort.audio")}</div>}
export function SignalPath({status,health,compact=false,horizontal=false}:React.ComponentProps<typeof LinuxSignalPath>){
 const{t}=useTranslation();const information=signalPathInformation(status,health);const format=information.format;
 const reported=format.kind==="pcm"?t("signalPath.pcm",{rate:format.rateKhz,bits:format.bits,channels:format.channels}):format.kind==="dsd"?t("signalPath.dsd",{level:format.level??"",rate:format.rateMhz,channels:format.channels}):t(format.kind==="unknown"?"signalPath.unknownFormat":"signalPath.noSignal");
 const state=t(`signalPath.${information.state}`);
 const direct=(status as (typeof status & {windows_mode?:string}))?.windows_mode==="direct";
 const eq=!direct&&status.shared_dsp?.in_path===true;const route=direct?"WASAPI Direct":eq?"CamillaDSP · WASAPI Shared":"WASAPI Shared";
 const facts=<div className="space-y-3 text-xs"><AudioSignalDiagram mode={direct?"direct":"shared"} equalized={eq}/><p>{t("signalPath.reported")}: {reported}</p>{eq&&status.shared_dsp?.verified&&<p>{t("windowsPort.eqPcm")}</p>}<OutputObservation status={status} health={health}/></div>;
 const help=<SignalPathInfo title={t("signalPath.modal.title")} openLabel={t("signalPath.modal.open")} closeLabel={t("signalPath.modal.close")}>{facts}</SignalPathInfo>;
 if(compact)return <section aria-label={t("signalPath.title")} data-signal-summary className="flex min-w-0 flex-col items-start gap-0.5 text-[10px] leading-tight">{help}<span className="max-w-full truncate text-[9px] text-[color:var(--theme-fg-secondary,#aaa)]" title={`${route} · ${reported} · ${state}`}>{route} · {state}</span></section>;
 return <section aria-label={t("signalPath.title")} data-signal-summary className={`relative w-full min-w-0 rounded-xl border border-[color:var(--theme-border-accent,#E5A00D)]/20 p-4 ${horizontal?"np-signal-path":""}`}>
  <div className="np-signal-heading flex flex-wrap items-center justify-between gap-2"><h3 className="text-xs font-semibold">{t("signalPath.title")}</h3><div className="flex items-center gap-2 text-[11px]"><span>{route} · {state}</span>{help}</div></div>
  <p className="np-signal-stages mt-3 text-xs">MPD → {route} → Windows</p>
  <div className="np-signal-metadata mt-3 text-[11px]"><p className="np-signal-format">{t("signalPath.reported")}: {reported}</p></div>
 </section>;
}
