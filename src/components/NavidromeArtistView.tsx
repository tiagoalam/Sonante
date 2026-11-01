import {useEffect,useRef,useState} from 'react';
import {ArrowLeft,LoaderCircle} from 'lucide-react';
import {useTranslation} from 'react-i18next';
import {navidromeService,navidromeError} from '../services/navidrome';
import type {NavidromeArtistDetail,NavidromeStatus} from '../types/navidrome';
import {NavidromeView} from './NavidromeView';
import {NavidromeDiscoveryCard} from './NavidromeDiscoveryCard';
import {VirtualAlbumGrid} from './VirtualAlbumGrid';
import {usePlaybackAttempt} from '../utils/usePlaybackAttempt';
import {PlaybackRequestNotice} from './PlaybackRequestNotice';
const button='rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 text-xs';
export function NavidromeArtistView({artist,onBack,backLabel,available,active=true,revision=0}:{artist:{account_id:string;id:string};onBack:()=>void;backLabel?:string;available:boolean;active?:boolean;revision?:number}){
 const {t}=useTranslation();
 const [data,setData]=useState<NavidromeArtistDetail|null>(null),[connection,setConnection]=useState<NavidromeStatus|null>(null),[error,setError]=useState<string|null>(null),[loading,setLoading]=useState(true),[retry,setRetry]=useState(0);
 const [album,setAlbum]=useState<string|null>(null),[container,setContainer]=useState<HTMLDivElement|null>(null);const position=useRef(0);
 const key=JSON.stringify(artist);const loaded=useRef<string|null>(null);
 const request=usePlaybackAttempt(key+String(active)+String(available)+String(album)+String(retry)+String(revision),'player.request',connection);
 useEffect(()=>{
  const visit=JSON.stringify([key,retry,revision]);if(!active||loaded.current===visit)return;
  let live=true;setLoading(true);setError(null);setData(null);setAlbum(null);setConnection(null);position.current=0;
  void (async()=>{const status=await navidromeService.status();if(!live)return;if(!status.connection_id||status.account_id!==artist.account_id)throw 'NAVIDROME_CONTEXT_CHANGED';const detail=await navidromeService.artist(status.connection_id,artist.id);if(live){setConnection(status);setData(detail);}})().catch(cause=>{if(live)setError(navidromeError(cause));}).finally(()=>{if(live){loaded.current=visit;setLoading(false);}});
  return()=>{live=false;};
 },[key,retry,revision,active]);
 useEffect(()=>{if(!album&&container)container.scrollTop=position.current;},[album,container]);
 if(album&&connection)return <NavidromeView embedded available={available&&active} initialAlbum={{account_id:artist.account_id,id:album}} onAlbumBack={()=>setAlbum(null)} backLabel={data?.artist.name}/>;
 return <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]">
  <header className="shrink-0 border-b border-[color:var(--theme-border-soft,#262626)] px-8 py-5"><button type="button" onClick={onBack} className="mb-5 flex items-center gap-2 text-sm text-[color:var(--theme-fg-secondary,#aaa)]"><ArrowLeft size={17}/>{backLabel??t('navidrome.back')}</button><p className="text-xs font-semibold uppercase tracking-wider text-[color:var(--theme-fg-accent,#E5A00D)]">{t('explore.sources.navidrome')}</p><h1 className="mt-2 text-2xl font-bold">{data?.artist.name??t('plex.artists')}</h1>{data&&<p className="mt-1 text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('plex.albumsCount',{count:data.albums.length})}</p>}</header>
  <div ref={setContainer} onScroll={event=>{position.current=event.currentTarget.scrollTop;}} className="min-h-0 flex-1 overflow-y-auto p-8">
   <PlaybackRequestNotice failure={request.failure}/>{loading&&<p role="status" className="flex items-center gap-2 text-sm"><LoaderCircle size={16} className="animate-spin"/>{t('navidrome.loading')}</p>}
   {error&&<div role="alert" className="space-y-3 text-sm"><p>{t(`navidrome.errors.${error}`)}</p><button type="button" className={button} onClick={()=>setRetry(v=>v+1)}>{t('navidrome.retry')}</button></div>}
   {data&&connection?.connection_id&&<VirtualAlbumGrid items={data.albums} getItemKey={a=>a.id} scrollContainer={container} renderItem={a=><NavidromeDiscoveryCard album={a} connectionId={connection.connection_id!} available={available&&active} onOpen={()=>setAlbum(a.id)} onPlay={()=>{if(!available||!active)return;void request.run(async intent=>{await navidromeService.album(connection.connection_id!,a.id);if(intent.current())await navidromeService.play(connection.connection_id!,a.id,0,intent);});}}/>}/>}
   {data&&!data.albums.length&&<p className="text-sm text-[color:var(--theme-fg-muted,#888)]">{t('navidrome.empty')}</p>}
  </div>
 </section>;
}
