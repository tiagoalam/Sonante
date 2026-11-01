import { Grid2X2, List, Rows3 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ALBUM_COVER_SIZES, type AlbumPresentation, type AlbumCoverSize } from "../utils/albumPresentation";
export function AlbumViewControls({ value, onChange }: { value: AlbumPresentation; onChange: (next: AlbumPresentation) => void }) {
  const { t }=useTranslation();
  return <div className="flex shrink-0 flex-wrap items-center gap-2">
    <div role="group" aria-label={t('albumViews.layout')} className="flex rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#1a1a1a)] p-0.5">
      {([['grid',Grid2X2],['list',List],['details',Rows3]] as const).map(([layout,Icon])=><button key={layout} type="button"
        title={t(`albumViews.${layout}`)} aria-label={t(`albumViews.${layout}`)} aria-pressed={value.layout===layout}
        onClick={()=>onChange({...value,layout})}
        className={`rounded-md p-1.5 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] ${value.layout===layout?'bg-[var(--theme-surface-accent,#332b15)] text-[color:var(--theme-fg-accent,#E5A00D)]':'text-[color:var(--theme-fg-muted,#999)] hover:bg-[var(--theme-surface-raised,#252525)]'}`}><Icon size={16} aria-hidden="true" /></button>)}
    </div>
    {value.layout==='grid'&&<label className="flex items-center gap-2 text-xs text-[color:var(--theme-fg-muted,#999)]"><span>{t('albumViews.covers')}</span>
      <select aria-label={t('albumViews.coverSize')} value={value.coverSize} onChange={e=>onChange({...value,coverSize:e.target.value as AlbumCoverSize})}
        className="rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#1a1a1a)] px-2 py-1.5 text-xs text-[color:var(--theme-fg-primary,#fff)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
        {ALBUM_COVER_SIZES.map(size=><option key={size} value={size}>{t(`albumViews.${size}`)}</option>)}
      </select>
    </label>}
  </div>;
}
