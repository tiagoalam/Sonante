import { useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Smartphone, LoaderCircle, QrCode, ShieldOff } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { BetaBadge } from './ExperimentalFeatures';
interface RemoteStatus {enabled:boolean;requested_enabled:boolean;port:number;urls:string[];code:string|null;expires_in:number;devices:string[]}
export function RemoteControlSetting(){
 const {t}=useTranslation(),id=useId();
 const [status,setStatus]=useState<RemoteStatus|null>(null),[enabled,setEnabled]=useState(false),[port,setPort]=useState('8787'),[url,setUrl]=useState('');
 const [busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null),[qr,setQr]=useState<string|null>(null);
 const generation=useRef(0),active=useRef(true),busyRef=useRef(false);
 const update=(next:RemoteStatus,draft=false)=>{setStatus(next);if(draft){setEnabled(next.requested_enabled);setPort(String(next.port));}setUrl(previous=>next.urls.includes(previous)?previous:next.urls[0]??'');};
 const read=async(initial=false)=>{const own=generation.current;if(initial)setError(null);try{const next=await invoke<RemoteStatus>('get_remote_control');if(active.current&&own===generation.current)update(next,initial);}catch{if(active.current&&own===generation.current)setError('unavailable');}};
 useEffect(()=>{active.current=true;void read(true);return()=>{active.current=false;generation.current++;};},[]);
 useEffect(()=>{if(!status?.enabled)return;const timer=setInterval(()=>{if(!busyRef.current)void read();},5000);return()=>clearInterval(timer);},[status?.enabled]);
 useEffect(()=>{setQr(null);if(!status?.enabled||!status.code||!url)return;let current=true;void invoke<string>('remote_control_qr',{url}).then(svg=>{if(current&&active.current)setQr(`data:image/svg+xml;base64,${btoa(svg)}`);}).catch(()=>{});return()=>{current=false;};},[status?.enabled,status?.code,url]);
 const run=async(command:string,args?:Record<string,unknown>)=>{if(busyRef.current)return;busyRef.current=true;setBusy(true);setError(null);const own=++generation.current;try{const next=await invoke<RemoteStatus>(command,args);if(active.current&&generation.current===own)update(next,command==='set_remote_control');}catch(cause){if(active.current&&generation.current===own){setError(cause==='REMOTE_PORT_UNAVAILABLE'?'port':'unavailable');void read();}}finally{if(active.current&&generation.current===own)setBusy(false);busyRef.current=false;}};
 const number=Number(port),valid=/^\d{4,5}$/u.test(port)&&number>=1024&&number<=65535;
 return <fieldset className="space-y-3 border-t border-[color:var(--theme-border-soft,#333333)] pt-4" aria-describedby={`${id}-help`}>
  <legend className="flex flex-wrap items-center gap-2 text-sm font-semibold"><Smartphone size={16}/>{t('remote.title')}<BetaBadge/></legend>
  <p id={`${id}-help`} className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t('remote.help')}</p>
  <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={enabled} disabled={busy||!status} onChange={event=>setEnabled(event.target.checked)} className="accent-[var(--theme-control-accent,#E5A00D)]"/>{t('remote.enable')}</label>
  <div className="flex flex-wrap items-end gap-3"><label className="space-y-2 text-xs"><span className="block">{t('remote.port')}</span><input type="number" min={1024} max={65535} value={port} onChange={event=>setPort(event.target.value)} disabled={busy||!status} className="w-28 rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm"/></label><button type="button" onClick={()=>status?void run('set_remote_control',{enabled,port:number}):void read(true)} disabled={busy||!!status&&(!valid||enabled===status.requested_enabled&&number===status.port&&status.enabled===status.requested_enabled)} className="rounded-lg border border-[color:var(--theme-border-accent,#E5A00D)] px-4 py-2 text-xs font-semibold text-[color:var(--theme-fg-accent,#E5A00D)] disabled:opacity-50">{busy?<LoaderCircle size={15} className="animate-spin" aria-label={t('remote.applying')}/>:t(status?'remote.apply':'remote.retry')}</button></div>
  <p className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t('remote.portNote',{port:valid?number:8787})}</p>
  {status&&<p role="status" className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t(status.enabled?'remote.active':'remote.inactive')}</p>}
  {status?.enabled&&<div className="space-y-3 rounded-xl bg-[var(--theme-surface-base,#121212)] p-4">
   <label className="block space-y-2 text-xs"><span>{t('remote.address')}</span><select aria-label={t('remote.address')} value={url} onChange={event=>setUrl(event.target.value)} className="w-full rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#181818)] px-3 py-2 font-mono text-sm">{status.urls.map(value=><option key={value} value={value}>{value}</option>)}</select></label>
   <input readOnly value={url} onFocus={event=>event.target.select()} aria-label={t('remote.copyAddress')} className="w-full rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-transparent px-3 py-2 font-mono text-xs"/>
   <div className="flex flex-wrap gap-2"><button type="button" disabled={busy||!enabled} onClick={()=>void run('pair_remote_control')} className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333333)] px-3 py-2 text-xs font-semibold disabled:opacity-50"><QrCode size={15}/>{t('remote.pair')}</button><button type="button" disabled={busy||!status.devices.length&&!status.code} onClick={()=>void run('revoke_remote_control')} className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333333)] px-3 py-2 text-xs disabled:opacity-50"><ShieldOff size={14}/>{t('remote.revoke')}</button></div>
   {status.code&&<div className="flex flex-wrap items-center gap-5 py-2">{qr&&<img src={qr} width={160} height={160} alt={t('remote.qrAlt')} className="rounded-lg bg-white p-2"/>}<div><p className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t('remote.code')}</p><p className="my-2 font-mono text-3xl tracking-[0.25em] text-[color:var(--theme-fg-accent,#E5A00D)]">{status.code}</p><p className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t('remote.expiry',{seconds:status.expires_in})}</p></div></div>}
   <p className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t('remote.devices',{count:status.devices.length})}{status.devices.length>0?` · ${status.devices.join(', ')}`:''}</p>
   <p className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t('remote.note')}</p>
  </div>}
  {error&&<p role="alert" className="text-xs text-[color:var(--theme-fg-error,#ff9999)]">{t(error==='port'?'remote.errors.port':'remote.errors.unavailable')}</p>}
 </fieldset>;
}
