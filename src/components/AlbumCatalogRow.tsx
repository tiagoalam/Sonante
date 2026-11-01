import type { ReactNode } from "react";
import { Heart, Play } from "lucide-react";
import { useTranslation } from "react-i18next";
import { albumAddedDate } from "../utils/albumPresentation";
interface Props {
  title: string; artist: string; year?: string; trackCount?: number | null; discCount?: number;
  genres?: readonly string[]; library?: string | null; addedAt?: string | null;
  detailed: boolean; artwork?: ReactNode; favorite: boolean; readable: boolean; playable: boolean; favoriteDisabled?: boolean;
  onOpen: ()=>void; onPlay: (event: React.MouseEvent)=>void; onFavorite: (event: React.MouseEvent)=>void;
  queueAction: ReactNode; artistAction?: ReactNode;
}
export function AlbumCatalogRow({title,artist,year,trackCount,discCount,genres,library,addedAt,detailed,artwork,favorite,readable,playable,favoriteDisabled=false,onOpen,onPlay,onFavorite,queueAction,artistAction}: Props) {
 const {t,i18n}=useTranslation(); const added=detailed?albumAddedDate(addedAt,i18n.language):null;
 return <div className="group flex h-full min-w-0 items-center gap-3 border-b border-[color:var(--theme-border-soft,#272727)] px-2 py-2 hover:bg-[var(--theme-surface-raised,#202020)] sm:px-3">
  <button type="button" disabled={!readable} onClick={onOpen} className="flex min-w-0 flex-1 items-center gap-3 rounded-lg text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-default">
   {!detailed&&<span className="h-14 w-14 shrink-0 overflow-hidden rounded-md bg-[var(--theme-surface-raised,#252525)]">{artwork}</span>}
   <span className="min-w-0 flex-1">
    <span className={detailed?'grid min-w-0 gap-x-4 sm:grid-cols-2':'block'}>
     <span className="block truncate text-sm font-semibold text-[color:var(--theme-fg-primary,#fff)]" title={title}>{title}</span>
     {!artistAction && <span className="block truncate text-xs text-[color:var(--theme-fg-secondary,#aaa)] sm:self-center" title={artist}>{artist}</span>}
    </span>
    <span className="mt-1 flex max-h-10 flex-wrap gap-x-3 gap-y-0.5 overflow-hidden text-[11px] text-[color:var(--theme-fg-muted,#888)]">
     {year&&<span>{year}</span>}{trackCount != null&&<span>{t('localBrowser.trackCount',{count:trackCount})}</span>}
     {detailed&&<>
      {!!discCount&&discCount>1&&<span>{t('albumViews.discs',{count:discCount})}</span>}
      {!!genres?.length&&<span className="max-w-52 truncate" title={genres.join(', ')}>{genres.slice(0,2).join(' · ')}</span>}
      {added&&<span>{t('albumViews.added',{date:added})}</span>}
      {library&&<span className="max-w-40 truncate" title={library}>{library}</span>}
     </>}
    </span>
   </span>
  </button>
  {artistAction && <div className="min-w-0 max-w-[30%] shrink text-xs">{artistAction}</div>}
  <div className="flex shrink-0 items-center gap-1">
   <button type="button" disabled={!playable} onClick={onPlay} aria-label={t('albumViews.play',{title})} title={t('albumViews.play',{title})} className="rounded-lg p-2 text-[color:var(--theme-fg-accent,#E5A00D)] hover:bg-[var(--theme-surface-accent,#332b15)] focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-40"><Play size={16} /></button>
   <button type="button" disabled={!readable||favoriteDisabled} onClick={onFavorite} aria-label={t(favorite?'favorites.removeFavorite':'common.addFavorite')} aria-pressed={favorite} className={`rounded-lg p-2 hover:bg-[var(--theme-surface-panel,#282828)] focus-visible:ring-2 focus-visible:ring-[#E5A00D] disabled:opacity-40 ${favorite?'text-[color:var(--theme-fg-accent,#E5A00D)]':'text-[color:var(--theme-fg-muted,#888)]'}`}><Heart size={16} fill={favorite?'currentColor':'none'} /></button>
   {queueAction}
  </div>
 </div>;
}
