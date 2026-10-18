import {useEffect,useRef,useState} from 'react';
import {useTranslation} from 'react-i18next';
import {equalizerService} from '../services/equalizer';
import type {DspSnapshot,EqComparison as Comparison} from '../types/equalizer';
export function EqComparison({snapshot,disabled,onChange,onBusy,sourceLabel}:{sourceLabel?:string;snapshot:DspSnapshot|undefined;disabled:boolean;onChange:(s:DspSnapshot)=>void;onBusy:(busy:boolean)=>void}){
 const {t}=useTranslation();const[working,setWorking]=useState(false),[error,setError]=useState(false);const alive=useRef(true),busy=useRef(false),owned=useRef<string|null>(null),latest=useRef<Comparison|null>(null),trigger=useRef<HTMLButtonElement>(null);
 latest.current=snapshot?.comparison??null;const comparison=latest.current;
 useEffect(()=>{alive.current=true;return()=>{alive.current=false;const c=latest.current;if(c&&owned.current===c.token&&!c.interrupted)void equalizerService.compare('finish',owned.current).catch(()=>{});onBusy(false)}},[]);
 const run=async(action:'reference'|'equalized'|'finish')=>{
  if(busy.current||disabled)return;busy.current=true;setWorking(true);setError(false);onBusy(true);
  const c=latest.current;
  try{const value=await equalizerService.compare(action,c?.token??null,c?null:snapshot?.applied??null);
   if(alive.current){latest.current=value.comparison??null;owned.current=value.comparison?.token??null;onChange(value);if(action==='finish'&&typeof requestAnimationFrame!=='undefined')requestAnimationFrame(()=>trigger.current?.focus())}
   else if(value.comparison&&!value.comparison.interrupted)void equalizerService.compare('finish',value.comparison.token).catch(()=>{});
  }catch{if(alive.current)setError(true)}finally{busy.current=false;onBusy(false);if(alive.current)setWorking(false)}
 };
 const locked=disabled||working;const button='rounded-lg border border-white/15 px-3 py-2 text-xs disabled:opacity-35';
 return <section aria-busy={working||snapshot?.phase==='updating'} aria-label={t('equalizer.compare.title')} onKeyDown={e=>{if(e.key==='Escape'&&comparison&&!comparison.interrupted&&!locked){e.preventDefault();void run('finish')}}} className="rounded-xl border border-white/10 p-4">
  <div className="flex flex-wrap items-center gap-2"><h2 className="mr-auto text-sm font-medium">{t('equalizer.compare.title')}</h2>
   {!comparison?<button ref={trigger} type="button" disabled={locked||!snapshot?.applied||!snapshot.verified||!!snapshot.error} onClick={()=>void run('reference')} className={button}>{t('equalizer.compare.start')}</button>:<>
    <button type="button" aria-pressed={comparison.side==='reference'} disabled={locked||comparison.interrupted} onClick={()=>void run('reference')} className={`${button} ${comparison.side==='reference'?'text-[#E5A00D] border-amber-500/60':''}`}>{t('equalizer.compare.reference')}</button>
    <button type="button" aria-pressed={comparison.side==='equalized'} disabled={locked||comparison.interrupted} onClick={()=>void run('equalized')} className={`${button} ${comparison.side==='equalized'?'text-[#E5A00D] border-amber-500/60':''}`}>{t('equalizer.compare.equalized')}</button>
    <button type="button" disabled={locked||comparison.interrupted} onClick={()=>void run('finish')} className={button}>{t('equalizer.compare.finish')}</button>
   </>}
  </div>
  <p className="mt-2 text-[11px] leading-relaxed text-white/50">{t('equalizer.compare.help')}</p>
  {comparison&&<p role="status" className="mt-2 text-xs text-amber-100">{working||snapshot?.phase==='updating'||!snapshot?.verified?t(`equalizer.state.${working?'updating':snapshot?.phase??'loading'}`):comparison.interrupted?t('equalizer.compare.interrupted'):t('equalizer.compare.current',{side:comparison.side==='reference'?'A':'B',name:sourceLabel??comparison.source.name,gain:comparison.effective_gain_db.toFixed(1)})}</p>}
  {error&&<p role="alert" className="mt-2 text-xs text-amber-100">{t('equalizer.compare.failed')}</p>}
 </section>;
}
