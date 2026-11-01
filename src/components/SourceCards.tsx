import { HardDrive, Server, Check } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { EnabledSources, MusicSource } from '../types/config';
import { MUSIC_SOURCES } from '../utils/musicSources';
export function SourceCards({ value, onChange, disabled = false }: { value: EnabledSources; onChange: (source: MusicSource, enabled: boolean) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  return <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
    {MUSIC_SOURCES.map(source => { const Icon = source === 'local' ? HardDrive : Server;
      return <button key={source} type="button" role="switch" aria-checked={value[source]} disabled={disabled}
        onClick={() => onChange(source, !value[source])}
        className={`flex min-w-0 flex-col gap-3 rounded-xl border p-4 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-50 ${value[source] ? 'border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]' : 'border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#141414)]'}`}>
        <span className="flex items-center gap-2 font-bold"><Icon size={18} className="shrink-0" /><span className="min-w-0 truncate">{t(`sources.names.${source}`)}</span></span>
        <span className="flex-1 text-xs text-[color:var(--theme-fg-muted,#999)]">{t(`sources.descriptions.${source}`)}</span>
        <span className="flex items-center justify-between text-xs font-semibold"><span>{t(value[source] ? 'wizard.enabled' : 'wizard.disabled')}</span><Check size={15} className={value[source] ? 'text-[color:var(--theme-fg-accent,#E5A00D)]' : 'opacity-20'} /></span>
      </button>;
    })}
  </div>;
}
