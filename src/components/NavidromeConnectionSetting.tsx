import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { navidromeService, navidromeError } from '../services/navidrome';
import type { NavidromeStatus } from '../types/navidrome';
const control='rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm disabled:opacity-50';
export function NavidromeConnectionSetting({ onBusyChange, onConfiguredChange }: { onBusyChange?: (busy: boolean) => void; onConfiguredChange?: (configured: boolean) => void }) {
  const { t }=useTranslation();
  const [status,setStatus]=useState<NavidromeStatus|null>(null), [server,setServer]=useState(''), [username,setUsername]=useState(''), [password,setPassword]=useState('');
  const [loading,setLoading]=useState(true),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null),[retry,setRetry]=useState(0);
  const alive=useRef(false), pending=useRef(false), notify=useRef(onBusyChange);notify.current=onBusyChange;
  const notifyConfigured=useRef(onConfiguredChange);notifyConfigured.current=onConfiguredChange;
  useEffect(()=>{alive.current=true;let live=true;setLoading(true);
    void navidromeService.status().then(value=>{if(live){setStatus(value);notifyConfigured.current?.(!!value.server && !!value.username);setServer(value.server??'');setUsername(value.username??'');setError(null);}})
      .catch(cause=>{if(live)setError(navidromeError(cause));}).finally(()=>{if(live)setLoading(false);});
    return()=>{live=false;alive.current=false;};
  },[retry]);
  async function save(disconnect=false){
    if(pending.current || (disconnect && !window.confirm(t('navidrome.disconnectConfirm'))))return;
    pending.current=true;setBusy(true);setError(null);notify.current?.(true);
    try{const next=disconnect?await navidromeService.disconnect():await navidromeService.connect(server,username,password);
      if(alive.current){setStatus(next);notifyConfigured.current?.(!!next.server && !!next.username);setPassword('');if(disconnect){setServer('');setUsername('');}}}
    catch(cause){if(alive.current)setError(navidromeError(cause));}
    finally{pending.current=false;if(alive.current)setBusy(false);notify.current?.(false);}
  }
  return <div className="space-y-3">
    {loading?<p role="status">{t('navidrome.loading')}</p>:<>
      <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('navidrome.connectionHint')}</p>
      {status?.server&&<p role="status" className="text-xs">{t('sources.connectionSaved')}</p>}
      {(['server','username','password'] as const).map(field=><label key={field} className="block space-y-1"><span>{t(`navidrome.${field}`)}</span>
        <input className={control+' w-full'} type={field==='password'?'password':'text'} autoComplete={field==='password'?'current-password':field==='username'?'username':'url'}
          value={field==='server'?server:field==='username'?username:password} disabled={busy}
          maxLength={field==='password'?4096:field==='server'?2048:256} onChange={e=>(field==='server'?setServer:field==='username'?setUsername:setPassword)(e.target.value)} />
      </label>)}
      <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('navidrome.credentialsHint')}</p>
      <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('sources.connectionApplies')}</p>
      <div className="flex flex-wrap gap-2"><button type="button" className={control} disabled={busy||!server.trim()||!username.trim()||!password} onClick={()=>void save()}>{t(busy?'navidrome.connecting':'navidrome.connect')}</button>
      {status?.server&&<button type="button" className={control} disabled={busy} onClick={()=>void save(true)}>{t('navidrome.disconnect')}</button>}</div>
    </>}
    {error&&<p role="alert">{t(`navidrome.errors.${error}`)} <button type="button" className={control} disabled={busy} onClick={()=>setRetry(n=>n+1)}>{t('navidrome.retry')}</button></p>}
  </div>;
}
