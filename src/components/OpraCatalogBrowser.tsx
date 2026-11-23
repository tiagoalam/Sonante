import {FormEvent,useEffect,useRef,useState} from 'react';
import {useTranslation} from 'react-i18next';
import {RefreshCw,Search,Headphones} from 'lucide-react';
import {equalizerService} from '../services/equalizer';
import type {OpraCatalogSearch,OpraProductHit} from '../types/equalizer';
import {opraLoadError} from '../utils/opraErrors';
import {classifyOpraProducts} from '../utils/opraSearch';

const compatibilityClass=(state:string)=>state==='compatible'?'border-emerald-500/30 text-emerald-200':state==='incompatible'?'border-rose-500/30 text-rose-200':'border-amber-500/30 text-amber-100';

export function OpraCatalogBrowser({disabled=false,registeredProductIds=[],onAddProduct}:{disabled?:boolean;registeredProductIds?:string[];onAddProduct?:(product:OpraProductHit)=>Promise<unknown>}) {
 const {t}=useTranslation();
 const [query,setQuery]=useState(''),[result,setResult]=useState<OpraCatalogSearch|null>(null),[resultQuery,setResultQuery]=useState(''),[selectedProductId,setSelectedProductId]=useState<string|null>(null),[working,setWorking]=useState(false),[error,setError]=useState(''),[addingProductId,setAddingProductId]=useState<string|null>(null);
 const generation=useRef(0),alive=useRef(true);
 useEffect(()=>{alive.current=true;return()=>{alive.current=false;generation.current++}},[]);
 const run=async(refresh:boolean)=>{
  const value=query.trim();
  if(value.length<2){setError('invalidQuery');return}
  const id=++generation.current;setWorking(true);setError('');if(resultQuery!==value){setResult(null);setSelectedProductId(null)}
  try{const response=await equalizerService.searchOpraProfiles(value,refresh);if(alive.current&&generation.current===id){setResult(response);setResultQuery(value);setSelectedProductId(null)}}
  catch(reason){if(alive.current&&generation.current===id)setError(opraLoadError(reason))}
  finally{if(alive.current&&generation.current===id)setWorking(false)}
 };
 const submit=(event:FormEvent)=>{event.preventDefault();void run(false)};
 const selected=result?.products.find(product=>product.product_id===selectedProductId)??null;
 const grouped=result?classifyOpraProducts(resultQuery,result.products):{direct:[],related:[]};
 const addProduct=async(product:OpraProductHit)=>{if(!onAddProduct||addingProductId)return;setAddingProductId(product.product_id);try{await onAddProduct(product)}finally{if(alive.current)setAddingProductId(null)}};
 const button='rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.15))] px-3 py-2 text-sm disabled:opacity-40';
 return <section aria-label={t('equalizer.library.opraTitle')} className="space-y-4 rounded-2xl border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.12))] bg-[color:var(--theme-surface-panel,rgba(255,255,255,.025))] p-4 sm:p-6">
  <div className="flex items-start gap-3"><span className="rounded-xl bg-[color:var(--theme-control-accent,rgba(229,160,13,.14))] p-2 text-[color:var(--theme-fg-accent,#E5A00D)]"><Headphones size={22} aria-hidden="true"/></span><div><h2 className="text-base font-semibold">{t('autoeq.searchHeading')}</h2><p className="mt-1 text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('autoeq.searchDescription')}</p></div></div>
  <div className="rounded-xl border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.1))] bg-[color:var(--theme-surface-base,rgba(0,0,0,.12))] p-3 sm:p-4">
   <form onSubmit={submit} className="flex flex-wrap items-end gap-2">
    <label className="min-w-48 flex-1 text-sm font-medium">{t('equalizer.library.opraSearchPrompt')}<input aria-label={t('equalizer.library.opraQuery')} value={query} placeholder={t('equalizer.library.opraQuery')} maxLength={160} disabled={disabled||working} onChange={event=>{const value=event.target.value;setQuery(value);if(value.trim()!==resultQuery){setResult(null);setSelectedProductId(null)}}} className="mt-2 w-full rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.2))] bg-[var(--theme-surface-panel,#141414)] p-3 text-base"/></label>
    <button type="submit" disabled={disabled||working||query.trim().length<2} className={`${button} self-end bg-[var(--theme-control-accent,#E5A00D)] font-semibold text-[color:var(--theme-control-on-accent,#111)]`}><Search size={15} className="mr-1 inline"/>{working?t('equalizer.library.opraBusy'):t('equalizer.library.opraSearch')}</button>
    <button type="button" disabled={disabled||working} onClick={()=>void run(true)} className={`${button} self-end`}><RefreshCw size={14} className="mr-1 inline"/>{t('equalizer.library.opraRefresh')}</button>
   </form>
   <p className="mt-2 text-xs text-[color:var(--theme-fg-muted,#999)]">{t('equalizer.library.opraSearchHint')}</p>
   <p className="mt-1 text-[11px] text-[color:var(--theme-fg-muted,#999)]">{t('equalizer.library.opraPrivacy')}</p>
  </div>
  <details className="text-xs text-[color:var(--theme-fg-muted,#999)]"><summary className="cursor-pointer">{t('equalizer.library.opraAttributionShort')}</summary><div className="mt-2 flex flex-wrap gap-2"><span>{t('equalizer.library.opraAttribution')}</span><a className="underline" href="https://github.com/opra-project/OPRA" target="_blank" rel="noreferrer">{t('equalizer.library.opraOpenSource')}</a></div></details>
  {error&&<p role="alert" data-opra-error={error} className="rounded-lg border border-[color:var(--theme-border-warning,rgba(229,160,13,.35))] p-3 text-sm text-[color:var(--theme-fg-accent,var(--color-amber-100))]">{error==='invalidQuery'?t('equalizer.library.opraInvalidQuery'):t(`equalizer.library.opraErrors.${error}`)}</p>}
  {result&&<div className="space-y-3" aria-live="polite">
   <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('equalizer.library.opraSnapshot',{sha:result.snapshot_sha256.slice(0,12)})} · {t('equalizer.library.opraLicense',{license:result.dataset_license})}</p>
   {!result.products.length&&<p className="rounded-xl border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.1))] p-4 text-sm">{t('equalizer.library.opraNoResults')}</p>}
   {grouped.direct.length>0&&<ProductResults products={grouped.direct} selectedId={selectedProductId} registeredIds={registeredProductIds} addingId={addingProductId} onSelect={setSelectedProductId} onAdd={onAddProduct?product=>void addProduct(product):undefined} t={t}/>}
   {grouped.related.length>0&&<details><summary className="cursor-pointer text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{t('autoeq.relatedResults',{count:grouped.related.length})}</summary><div className="mt-2"><ProductResults products={grouped.related} selectedId={selectedProductId} registeredIds={registeredProductIds} addingId={addingProductId} onSelect={setSelectedProductId} onAdd={onAddProduct?product=>void addProduct(product):undefined} t={t}/></div></details>}
   {selected&&<section data-opra-selected-product={selected.product_id} aria-label={t('equalizer.library.opraDetailsTitle')} className="rounded-xl border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.12))] p-4">
    <div className="flex flex-wrap items-baseline justify-between gap-2"><div><p className="text-xs text-[color:var(--theme-fg-accent,#E5A00D)]">{selected.vendor_name}</p><h3 className="text-lg font-semibold">{selected.product_name}</h3></div><span className="text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{t('equalizer.library.opraProfilesCount',{count:selected.profile_count??selected.profiles.length})}</span></div>
    {onAddProduct&&<button type="button" disabled={disabled||addingProductId!==null||registeredProductIds.includes(selected.product_id)} onClick={()=>void addProduct(selected)} className={`${button} mt-3`}>{registeredProductIds.includes(selected.product_id)?t('autoeq.alreadyInMyEquipment'):addingProductId===selected.product_id?t('autoeq.addingEquipment'):t('autoeq.addToMyEquipment')}</button>}
    <div className="mt-3 space-y-2">{selected.profiles.map(profile=><details key={profile.profile_id} className="rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.1))] p-3">
     <summary className="cursor-pointer text-sm font-medium">{profile.author}{profile.details?` · ${profile.details}`:''}<span className={`ml-2 inline-block rounded-full border px-2 py-0.5 align-middle text-[10px] ${compatibilityClass(profile.compatibility)}`}>{t(profile.compatibility==='compatible'?'equalizer.library.opraStatusCompatibleShort':profile.compatibility==='incompatible'?'equalizer.library.opraStatusIncompatibleShort':'equalizer.library.opraStatusUnverifiedShort')}</span></summary>
     <div className="mt-3 space-y-2 text-xs">
      <p>{t('equalizer.library.opraProfileId',{id:profile.profile_id})}</p><p>{t('equalizer.library.opraAuthor',{author:profile.author})}</p><p>{t('equalizer.library.opraOverallGain',{gain:profile.overall_gain_db})}</p>
      {profile.source_link&&<p className="break-all text-[color:var(--theme-fg-secondary,rgb(255_255_255_/_0.65))]">{profile.source_link}</p>}
      <p>{t('equalizer.library.opraCompatibility',{state:t(`equalizer.library.opra${profile.compatibility==='compatible'?'Compatible':profile.compatibility==='incompatible'?'Incompatible':'Unverified'}`)})}</p>
      <p className="text-[color:var(--theme-fg-muted,#999)]">{t('equalizer.library.opraConversionBlocked')}</p>
      {profile.compatibility_reasons.length>0&&<ul className="list-disc pl-5">{profile.compatibility_reasons.map((reason,index)=><li key={`${reason}-${index}`}>{t(`equalizer.library.opraReasons.${reason}`)}</li>)}</ul>}
      <p className="pt-1 font-medium">{t('equalizer.library.opraBands')}</p>
      <div className="max-h-40 overflow-auto"><table className="w-full text-left"><thead><tr><th>#</th><th>{t('equalizer.library.autoEqType')}</th><th>{t('equalizer.frequency')}</th><th>{t('equalizer.gain')}</th><th>{t('equalizer.q')}</th><th>{t('equalizer.library.opraSlope')}</th></tr></thead><tbody>{profile.bands.map((band,index)=><tr key={`${index}-${band.frequency}`} className="border-t border-white/5"><td>{index+1}</td><td>{band.filter_type}</td><td>{band.frequency} {t('equalizer.hzUnit')}</td><td>{band.gain_db??'—'} {t('equalizer.dbUnit')}</td><td>{band.q??'—'}</td><td>{band.slope??'—'}</td></tr>)}</tbody></table></div>
     </div>
    </details>)}</div>
   </section>}
   {result.truncated&&<p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('equalizer.library.opraTruncated')}</p>}
  </div>}
 </section>;
}

export function ProductResults({products,selectedId,registeredIds,addingId,onSelect,onAdd,t}:{products:OpraProductHit[];selectedId:string|null;registeredIds:string[];addingId:string|null;onSelect:(id:string)=>void;onAdd?:((product:OpraProductHit)=>void);t:(key:string,options?:Record<string,unknown>)=>string}) {
 const button='rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.15))] px-3 py-2 text-xs disabled:opacity-40';
 return <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3" role="group" aria-label={t('autoeq.searchResults')}>
  {products.map(product=><article key={product.product_id} data-opra-product={product.product_id} className={`rounded-xl border p-3 transition-colors ${selectedId===product.product_id?'border-[color:var(--theme-control-accent,#E5A00D)] bg-[color:var(--theme-control-accent,rgba(229,160,13,.1))]':'border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.12))] hover:bg-white/5'}`}>
   <button type="button" aria-pressed={selectedId===product.product_id} aria-label={t('equalizer.library.opraSelectProduct',{product:product.product_name})} onClick={()=>onSelect(product.product_id)} className="block w-full text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"><span className="block text-[11px] font-semibold uppercase tracking-wide text-[color:var(--theme-fg-accent,#E5A00D)]">{product.vendor_name||t('autoeq.unknownManufacturer')}</span><span className="mt-1 block text-base font-semibold text-[color:var(--theme-fg-primary,#fff)]">{product.product_name}</span><span className="mt-1 block text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{t('equalizer.library.opraProfileCount',{count:product.profile_count??product.profiles.length})}</span></button>
   {onAdd&&<button type="button" disabled={registeredIds.includes(product.product_id)||addingId!==null} onClick={()=>onAdd(product)} className={`${button} mt-3`}>{registeredIds.includes(product.product_id)?t('autoeq.alreadyInMyEquipment'):addingId===product.product_id?t('autoeq.addingEquipment'):t('autoeq.addToMyEquipment')}</button>}
  </article>)}
 </div>;
}
