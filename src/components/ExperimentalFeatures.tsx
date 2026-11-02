import { createContext, useContext, useId } from 'react';
import { FlaskConical } from 'lucide-react';
import { useTranslation } from 'react-i18next';
export const ExperimentalFeaturesContext = createContext(false);
export const useExperimentalFeatures = () => useContext(ExperimentalFeaturesContext);
export function BetaBadge() {
  const { t } = useTranslation();
  return <span className="inline-flex shrink-0 items-center rounded-md border border-[color:var(--theme-border-accent,#E5A00D)] px-1.5 py-0.5 text-[10px] font-semibold tracking-wide text-[color:var(--theme-fg-accent,#E5A00D)]">{t('experimental.beta')}</span>;
}
export function ExperimentalNotice() {
  const { t } = useTranslation();
  return <p className="mt-3 max-w-3xl text-xs leading-relaxed text-[color:var(--theme-fg-muted,#999)]">{t('experimental.notice')}</p>;
}
export function ExperimentalFeaturesSetting({ enabled, onChange, disabled = false }: { enabled: boolean; onChange: (value: boolean) => void; disabled?: boolean }) {
  const { t } = useTranslation(); const id = useId();
  return <fieldset className="space-y-3 border-t border-[color:var(--theme-border-soft,#333333)] pt-4" aria-describedby={`${id}-description ${id}-features`}>
    <legend className="flex flex-wrap items-center gap-2 text-sm font-semibold"><FlaskConical size={16} aria-hidden="true" />{t('experimental.title')}<BetaBadge /></legend>
    <p id={`${id}-description`} className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t('experimental.description')}</p>
    <label className="flex items-center gap-2 text-sm">
      <input type="checkbox" checked={enabled} disabled={disabled} onChange={event => onChange(event.target.checked)} aria-describedby={`${id}-description ${id}-features`} className="accent-[var(--theme-control-accent,#E5A00D)]" />
      {t('experimental.enable')}
    </label>
    <p id={`${id}-features`} className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t('experimental.features')}</p>
  </fieldset>;
}
