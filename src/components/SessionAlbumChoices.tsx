import { useEffect, useId, useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { LoaderCircle, RefreshCw, X } from 'lucide-react';
import type { SessionCandidate } from '../types/listeningSession';

const button = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 text-xs font-semibold hover:bg-[var(--theme-surface-hover,#292929)] disabled:opacity-40 focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';
export function SessionAlbumChoices({ title, candidates, busy, more, error, onChoose, onMore, onClose, renderArtwork }: {
  title: string; candidates: readonly SessionCandidate[]; busy: boolean; more: boolean; error: string | null;
  onChoose: (candidate: SessionCandidate) => void; onMore: () => void; onClose: () => void;
  renderArtwork: (candidate: SessionCandidate) => ReactNode;
}) {
  const { t } = useTranslation(), id = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  return <section aria-labelledby={id} className="basis-full border-t border-[color:var(--theme-border-soft,#333)] pt-4"
    onKeyDown={event => { if (event.key === 'Escape' && !busy) { event.preventDefault(); event.stopPropagation(); onClose(); } }}>
    <div className="mb-3 flex items-start justify-between gap-3">
      <div><h4 ref={heading} id={id} tabIndex={-1} className="text-sm font-semibold outline-none">{t('sessions.replaceTitle', { title })}</h4>
        <p className="mt-1 text-xs text-[color:var(--theme-fg-muted,#999)]">{t('sessions.replaceHint')}</p></div>
      <button type="button" className={button} disabled={busy} aria-label={t('sessions.closeChoices')} onClick={onClose}><X size={14} /></button>
    </div>
    {error && <p role="alert" className="mb-3 text-xs text-[color:var(--theme-fg-error,#ff9999)]">{error}</p>}
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">{candidates.map(candidate => <button key={candidate.id} type="button" disabled={busy}
      onClick={() => onChoose(candidate)} aria-label={t('sessions.chooseReplacement', { title: candidate.title })}
      className="min-w-0 rounded-xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] p-3 text-left hover:border-[color:var(--theme-border-accent,#E5A00D)] disabled:opacity-40 focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
      <div className="mx-auto aspect-square w-full max-w-32 overflow-hidden rounded-lg">{renderArtwork(candidate)}</div>
      <p className="mt-2 truncate text-xs font-semibold" title={candidate.title}>{candidate.title}</p>
      <p className="mt-1 truncate text-[11px] text-[color:var(--theme-fg-muted,#999)]" title={candidate.artist}>{candidate.artist}</p>
      {candidate.year && <p className="mt-1 text-[11px] text-[color:var(--theme-fg-muted,#999)]">{candidate.year}</p>}
      <span className="mt-3 block text-xs font-semibold text-[color:var(--theme-fg-accent,#E5A00D)]">{t('sessions.chooseAlbum')}</span>
    </button>)}</div>
    <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
      {more && <button type="button" className={button} disabled={busy} onClick={onMore}><RefreshCw size={13} />{t('sessions.moreChoices')}</button>}
      {busy && <p role="status" className="inline-flex items-center gap-2 text-xs text-[color:var(--theme-fg-muted,#999)]"><LoaderCircle size={14} className="animate-spin" />{t('sessions.loadingChoice')}</p>}
    </div>
  </section>;
}
