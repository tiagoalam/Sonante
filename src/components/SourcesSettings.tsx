import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { AppConfig, MusicSource } from '../types/config';
import { enabledSources, sourceEnabled, withSourceEnabled, MUSIC_SOURCES } from '../utils/musicSources';
import { SourceCards } from './SourceCards';
import { NavidromeConnectionSetting } from './NavidromeConnectionSetting';
export function SourcesSettings({ config, onChange, initialSource, disabled, local, plex, onConnectionBusy, localCommitted }: {
 config: AppConfig; onChange: (config: AppConfig) => void; initialSource?: MusicSource;
 localCommitted: boolean; disabled: boolean; local: ReactNode; plex: ReactNode; onConnectionBusy: (busy:boolean)=>void;
}) {
 const { t }=useTranslation(); const [selected,setSelected]=useState<MusicSource>(initialSource??'local');const section=useRef<HTMLDivElement>(null);
 useEffect(()=>{if(initialSource)section.current?.scrollIntoView({block:'nearest'});},[]);
 return <div className="space-y-5">
  <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('sources.hint')}</p>
  <SourceCards value={enabledSources(config)} disabled={disabled} onChange={(source,enabled)=>{onChange(withSourceEnabled(config,source,enabled));setSelected(source);}} />
  <div className="flex flex-wrap gap-2" aria-label={t('sources.configure')}>
   {MUSIC_SOURCES.map(source=><button type="button" key={source} aria-pressed={selected===source} onClick={()=>setSelected(source)} className={`rounded-lg border px-3 py-2 ${selected===source?'border-[#E5A00D] text-[color:var(--theme-fg-accent,#E5A00D)]':'border-[color:var(--theme-border-soft,#333)]'}`}>{t(`sources.names.${source}`)}</button>)}
  </div>
  <div ref={section} className="rounded-xl border border-[color:var(--theme-border-soft,#333)] p-4">
   <h4 className="mb-3 text-sm font-bold">{t(`sources.names.${selected}`)}</h4>
   {!sourceEnabled(config,selected)?<p>{t('sources.disabledHint')}</p>:<>
    <div hidden={selected!=='local'}>{selected==='local'&&(localCommitted ? local : <p>{t('sources.saveBeforeFolders')}</p>)}</div>
    <div hidden={selected!=='plex'}>{selected==='plex'&&plex}</div>
    {selected==='navidrome'&&<NavidromeConnectionSetting onBusyChange={onConnectionBusy} />}
   </>}
  </div>
 </div>;
}
