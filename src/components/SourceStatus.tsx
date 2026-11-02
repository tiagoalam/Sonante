import { CheckCircle2, CircleAlert, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { SourceReadiness } from '../utils/sourceReadiness';
export function SourceStatus({ state }: { state: SourceReadiness }) {
  const { t } = useTranslation();
  if (state === 'disabled') return null;
  const ready = state === 'ready', working = state === 'checking' || state === 'busy';
  return <span role="img" aria-label={t(`sourceStatus.${state}`)} title={t(`sourceStatus.${state}`)} className={ready
    ? 'flex items-center text-[color:var(--theme-fg-success,#4BB543)]' : 'flex items-center text-[color:var(--theme-fg-accent,#E5A00D)]'}>
    {ready ? <CheckCircle2 size={13} /> : working ? <RefreshCw size={13} className="animate-spin" /> : <CircleAlert size={13} />}
  </span>;
}
