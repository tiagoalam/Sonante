import {useEffect,useRef,useState} from 'react';
import {LoaderCircle,RefreshCw} from 'lucide-react';
import {useTranslation} from 'react-i18next';
import {navidromeService,navidromeError} from '../services/navidrome';
import type {NavidromeAlbum,NavidromeExplore,NavidromeStatus} from '../types/navidrome';
import {NavidromeView} from './NavidromeView';
import {NavidromeArtistView} from './deferredViews';
import {NavidromeDiscoveryCard} from './NavidromeDiscoveryCard';
import {usePlaybackAttempt} from '../utils/usePlaybackAttempt';
import {PlaybackRequestNotice} from './PlaybackRequestNotice';
export function NavidromeExploreView({available,active=true,revision=0,onConfigure}:{available:boolean;active?:boolean;revision?:number;onConfigure:()=>void}){
 const {t}=useTranslation();const [connection,setConnection]=useState<NavidromeStatus|null>(null),[data,setData]=useState<NavidromeExplore|null>(null),[favorites,setFavorites]=useState<NavidromeAlbum[]>([]),[favoriteError,setFavoriteError]=useState(false),[error,setError]=useState<string|null>(null),[loading,setLoading]=useState(true),[retry,setRetry]=useState(0),[album,setAlbum]=useState<string|null>(null);
 const [artistTarget,setArtistTarget]=useState<{account_id:string;id:string}|null>(null);
 const loaded=useRef<string|null>(null);const position=useRef(0);const [container,setContainer]=useState<HTMLDivElement|null>(null);
 const request=usePlaybackAttempt(String(active)+String(available)+String(album)+String(retry)+String(revision),'player.request',connection);
 useEffect(()=>{
  const visit=JSON.stringify([retry,revision]);if(!active||loaded.current===visit)return;
  let live=true;position.current=0;setLoading(true);setData(null);setConnection(null);setFavorites([]);setFavoriteError(false);setError(null);setAlbum(null);
  void (async()=>{const status=await navidromeService.status();if(!live)return;setConnection(status);if(!status.connection_id)return;
   const [result,saved]=await Promise.allSettled([navidromeService.explore(status.connection_id),navidromeService.favorites(status.connection_id)]);if(!live)return;
   if(result.status==='fulfilled')setData(result.value);else setError(navidromeError(result.reason));
   if(saved.status==='fulfilled')setFavorites(saved.value.slice(0,18));else setFavoriteError(true);
  })().catch(cause=>{if(live)setError(navidromeError(cause));}).finally(()=>{if(live){loaded.current=visit;setLoading(false);}});
  return()=>{live=false;};
 },[retry,revision,active]);
 useEffect(()=>{if(!album&&container)container.scrollTop=position.current;},[album,container]);
 if(album&&connection?.account_id)return <NavidromeView embedded available={available&&active} initialAlbum={{id:album,account_id:connection.account_id}} onAlbumBack={()=>setAlbum(null)} backLabel={t('explore.title')}/>;
 const shelves=[{title:t('navidrome.exploreSuggested'),albums:data?.suggested??[]},{title:t('navidrome.exploreAdded'),albums:data?.added??[]},{title:t('navidrome.exploreFavorites'),albums:favorites}];
 return <>{artistTarget&&<NavidromeArtistView artist={artistTarget} available={available&&active} active={active} revision={revision} onBack={()=>setArtistTarget(null)}/>}
 <section hidden={!!artistTarget} className={`${artistTarget?'hidden':'flex'} min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]`}>
  <header className="flex shrink-0 flex-wrap items-center justify-between gap-4 border-b border-[color:var(--theme-border-soft,#262626)] px-8 py-6"><div><h1 className="text-2xl font-bold">{t('explore.title')}</h1><p className="mt-1 text-sm text-[color:var(--theme-fg-muted,#888)]">{t('navidrome.exploreSubtitle')}</p></div><button type="button" disabled={loading||!active} onClick={()=>setRetry(v=>v+1)} className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 text-xs disabled:opacity-40"><RefreshCw size={14}/>{t('explore.renew')}</button></header>
  <div ref={setContainer} onScroll={event=>{position.current=event.currentTarget.scrollTop;}} className="min-h-0 flex-1 overflow-y-auto p-8"><PlaybackRequestNotice failure={request.failure}/>
   {loading&&<p role="status" className="flex items-center gap-2 text-sm"><LoaderCircle size={16} className="animate-spin"/>{t('navidrome.loading')}</p>}
   {error&&<p role="alert" className="mb-5 text-sm text-[color:var(--theme-fg-error,#f99)]">{t(`navidrome.errors.${error}`)}</p>}
   {!loading&&!error&&!connection?.connection_id&&<div className="space-y-4 text-sm"><p>{t('sources.navidromeConfigureHint')}</p><button type="button" className="rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2" onClick={onConfigure}>{t('sources.configure')}</button></div>}
   {favoriteError&&<p role="alert" className="mb-5 text-sm">{t('navidrome.favoritesFailed')}</p>}
   {connection?.connection_id&&shelves.map(shelf=>shelf.albums.length>0&&<section key={shelf.title} className="mb-10"><h2 className="mb-5 text-lg font-semibold">{shelf.title}</h2><div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">{shelf.albums.map(a=><NavidromeDiscoveryCard key={a.id} album={a} connectionId={connection.connection_id!} available={available&&active} accountId={connection.account_id??undefined} active={active&&!artistTarget} onArtist={setArtistTarget} onOpen={()=>setAlbum(a.id)} onPlay={()=>{if(!available||!active)return;void request.run(async intent=>{await navidromeService.album(connection.connection_id!,a.id);if(intent.current())await navidromeService.play(connection.connection_id!,a.id,0,intent);});}}/>)}</div></section>)}
   {!loading&&connection?.connection_id&&data&&!data.added.length&&!data.suggested.length&&!favorites.length&&<p className="text-sm text-[color:var(--theme-fg-muted,#888)]">{t('navidrome.empty')}</p>}
  </div>
 </section></>;
}
