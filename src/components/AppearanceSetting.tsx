import {useId,useSyncExternalStore} from 'react';
import {useTranslation} from 'react-i18next';
import {appearance} from '../services/appearance';
export function AppearanceSetting(){
 const {t}=useTranslation();const id=useId();const value=useSyncExternalStore(appearance.subscribe,appearance.getSnapshot,()=> 'dark' as const);
 return <section className="space-y-3"><label className="block text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider" htmlFor={id}>{t('settings.appearance')}</label>
  <select id={id} value={value} onChange={e=>appearance.set(e.target.value==='light'?'light':'dark')} className="w-full rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-base,#121212)] px-3 py-2.5 text-sm text-[color:var(--theme-fg-primary,#ffffff)]">
   <option value="dark">{t('settings.appearanceDark')}</option><option value="light">{t('settings.appearanceLight')}</option>
  </select><p className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t('settings.appearanceHelp')}</p></section>;
}
