import {Play} from 'lucide-react';
import {useTranslation} from 'react-i18next';
import {NavidromeArtistLink} from './NavidromeArtistLink';
import {Artwork} from './NavidromeView';
import type {NavidromeAlbum} from '../types/navidrome';
export function NavidromeDiscoveryCard({album,connectionId,available,onOpen,onPlay,accountId,onArtist,active=true}:{album:NavidromeAlbum;connectionId:string;available:boolean;onOpen:()=>void;onPlay:()=>void;active?:boolean;accountId?:string;onArtist?:(artist:{account_id:string;id:string})=>void}){
 const {t}=useTranslation();
 return <div className="group relative flex min-w-0 flex-col">
  <div className="relative mb-2.5"><button type="button" className="block w-full" aria-label={t('navidrome.openAlbum',{title:album.title})} onClick={onOpen}><Artwork connection={connectionId} id={album.cover_id}/></button>
   <button type="button" onClick={onPlay} disabled={!available||(album.song_count??0)>500} aria-label={t('navidrome.quickPlay',{title:album.title})} className="absolute bottom-3 right-3 flex h-11 w-11 items-center justify-center rounded-full bg-[#E5A00D] text-black opacity-0 shadow-lg transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 disabled:opacity-30"><Play size={19} fill="currentColor"/></button></div>
  <button type="button" onClick={onOpen} className="truncate text-left text-sm font-semibold" title={album.title}>{album.title}</button>
  <p className="mt-0.5 truncate text-xs text-[color:var(--theme-fg-secondary,#999)]" title={album.artist}>{accountId&&onArtist?<NavidromeArtistLink artist={album.artist} albumId={album.id} connectionId={connectionId} accountId={accountId} onOpen={onArtist} disabled={!active}/>:album.artist}</p>
  <p className="mt-0.5 text-[11px] text-[color:var(--theme-fg-muted,#777)]">{album.year??''}{album.year&&album.song_count!=null?' · ':''}{album.song_count!=null?t('navidrome.tracks',{count:album.song_count}):''}</p>
 </div>;
}
