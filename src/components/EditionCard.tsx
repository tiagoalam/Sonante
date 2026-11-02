import { ArrowUpRight } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { EditionEntry } from '../utils/albumEditions';
import { editionDisplayTitle, type EditionProfile } from '../utils/albumEditionAnalysis';
import { EditionArtwork } from './EditionArtwork';

export function EditionCard({ entry, active, updating, onOpen, copies = [entry], profile, checked = false, onOpenCopy }: { entry: EditionEntry; active: boolean; updating: boolean; onOpen: (trigger: HTMLButtonElement) => void;
  copies?: readonly EditionEntry[]; profile?: EditionProfile | null; checked?: boolean; onOpenCopy?: (entry: EditionEntry, trigger: HTMLButtonElement) => void }) {
  const { t } = useTranslation();
  const count = profile?.titles?.length ?? profile?.sizes?.length ?? (entry.source === 'local' ? entry.album.track_count : entry.source === 'navidrome' ? entry.album.song_count : null);
  return <article className="flex min-w-0 flex-col overflow-hidden rounded-2xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#181818)]">
    <div className="flex items-center gap-4 p-5"><div className="h-24 w-24 shrink-0 overflow-hidden rounded-xl"><EditionArtwork entry={entry} copies={copies} active={active} updating={updating} coverId={profile?.cover_id} /></div>
      <div className="min-w-0"><h3 className="break-words text-base font-bold">{editionDisplayTitle(entry, profile)}</h3><p className="mt-1 text-xs text-[color:var(--theme-fg-muted,#999)]">{entry.album.artist}</p><p className="mt-2 text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{entry.album.year || t('editions.unknown')}{count != null ? ` · ${t('editions.trackCount', { count })}` : ''}</p></div>
    </div>
    <div className="px-5 pb-5"><p className="mb-2 text-xs text-[color:var(--theme-fg-muted,#999)]">{t('editions.availableIn')}</p><div className="flex flex-wrap gap-2">{copies.map(copy => <button type="button" key={copy.key} title={copy.source === 'local' ? copy.album.folder_path : copy.origin} disabled={!active}
      onClick={event => onOpenCopy ? onOpenCopy(copy, event.currentTarget) : onOpen(event.currentTarget)}
      aria-label={t('editions.openNamed', { title: copy.album.title, source: t(`smart.sources.${copy.source}`) })}
      className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#444)] px-3 py-2 text-xs font-semibold hover:border-[color:var(--theme-border-accent,#E5A00D)] focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:opacity-40">{t(`smart.sources.${copy.source}`)}{copy.origin ? ` · ${copy.origin}` : ''}<ArrowUpRight size={12} /></button>)}</div></div>
    <details className="mt-auto border-t border-[color:var(--theme-border-soft,#333)] px-5 py-3 text-xs text-[color:var(--theme-fg-muted,#999)]"><summary className="cursor-pointer py-1">{t('editions.comparisonDetails')}</summary>
      <dl className="mt-3 space-y-3"><div><dt>{t('editions.fileBytes')}</dt><dd className="mt-1 tabular-nums">{profile?.sizes ? t('editions.byteTotal', { bytes: profile.sizes.reduce((sum, bytes) => sum + BigInt(bytes), 0n).toLocaleString() }) : t(checked ? 'editions.sizeUnavailable' : 'editions.sizePending')}</dd></div>
        {entry.source === 'local' && <div><dt>{t('editions.folder')}</dt><dd className="mt-1 break-all">{entry.album.folder_path}</dd></div>}</dl>
      <p className="mt-3">{t(copies.length > 1 ? 'editions.sameSizesHint' : 'editions.differentSizesHint')}</p>
    </details>
  </article>;
}
