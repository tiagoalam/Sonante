import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import type { UsageMode } from '../utils/usageMode';
export function UsageModeSetting({ value, onChange, disabled = false }: { value?: UsageMode; onChange: (mode: UsageMode) => void; disabled?: boolean }) {
  const { t } = useTranslation(); const id = useId();
  return <fieldset disabled={disabled} className="space-y-3 border-t border-[color:var(--theme-border-soft,#333)] pt-4" aria-describedby={`${id}-help`}>
    <legend className="text-sm font-semibold">{t('usageMode.title')}</legend>
    <div className="grid gap-3 sm:grid-cols-2">{(['simple', 'full'] as const).map(mode => <label key={mode} className={`flex cursor-pointer items-start gap-3 rounded-xl border p-4 ${((value ?? 'full') === mode) ? 'border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]' : 'border-[color:var(--theme-border-soft,#333)]'}`}>
      <input type="radio" name={id} value={mode} checked={(value ?? 'full') === mode} onChange={() => onChange(mode)} className="mt-1 accent-[var(--theme-control-accent,#E5A00D)]" />
      <span><span className="block text-sm font-semibold">{t(`usageMode.${mode}`)}</span><span className="mt-1 block text-xs leading-relaxed text-[color:var(--theme-fg-secondary,#aaa)]">{t(`usageMode.${mode}Description`)}</span></span>
    </label>)}</div>
    <p id={`${id}-help`} className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888)]">{t('usageMode.help')}</p>
  </fieldset>;
}
