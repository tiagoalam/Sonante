import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { navidromeService, navidromeError } from '../services/navidrome';
export function NavidromeArtistLink({artist,albumId,connectionId,accountId,onOpen,disabled=false,className=''}:{
 artist:string;albumId:string;connectionId:string;accountId:string;onOpen:(artist:{account_id:string;id:string})=>void;disabled?:boolean;className?:string;
}) {
 const {t}=useTranslation();const [pending,setPending]=useState(false),[error,setError]=useState<string|null>(null);
 const sequence=useRef(0),busy=useRef(false),alive=useRef(true);
 const identity=JSON.stringify([connectionId,accountId,albumId]);const current=useRef(identity);current.current=identity;const blocked=useRef(disabled);blocked.current=disabled;
 useEffect(()=>{alive.current=true;sequence.current++;busy.current=false;setPending(false);setError(null);return()=>{alive.current=false;sequence.current++;};},[identity,disabled]);
 async function open(){if(disabled||busy.current||!/^[a-f0-9]{64}$/.test(accountId))return;const key=identity,request=++sequence.current;busy.current=true;setPending(true);setError(null);
  try{const detail=await navidromeService.album(connectionId,albumId);if(!alive.current||blocked.current||current.current!==key||request!==sequence.current)return;
   if(detail.album.id!==albumId||!detail.artist_id)throw 'NAVIDROME_NAVIGATION_UNAVAILABLE';
   onOpen({account_id:accountId,id:detail.artist_id});
  }catch(cause){if(alive.current&&current.current===key&&request===sequence.current)setError(cause==='NAVIDROME_NAVIGATION_UNAVAILABLE'?'navigation':navidromeError(cause));}
  finally{if(alive.current&&current.current===key&&request===sequence.current){busy.current=false;setPending(false);}}
 }
 return <span className="block min-w-0"><button type="button" className={`max-w-full truncate text-left hover:underline focus-visible:underline disabled:opacity-50 ${className}`} disabled={disabled||pending||!artist||!/^[a-f0-9]{64}$/.test(accountId)} aria-busy={pending} title={artist} onClick={()=>void open()}>{artist}</button>{error&&<span role="alert" className="block text-[11px] text-[color:var(--theme-fg-error,#f99)]">{t(error==='navigation'?'player.navigationUnavailable':`navidrome.errors.${error}`)}</span>}</span>;
}
