import {useCallback,useEffect,useRef,useState,type FormEvent} from 'react';
import {useTranslation} from 'react-i18next';
import {Box,Headphones,Plus,Speaker,Sparkles,Trash2} from 'lucide-react';
import {equalizerService} from '../services/equalizer';
import type {EquipmentCategory,EquipmentProfile,EqCatalog,EqEdit,OpraProductHit,PresetRef} from '../types/equalizer';
import {OpraCatalogBrowser} from './OpraCatalogBrowser';

const refKey=(ref:PresetRef)=>`${ref.kind}:${ref.id}`;
const input='mt-1 w-full rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.18))] bg-[var(--theme-surface-base,#141414)] px-3 py-2 text-sm';
const button='inline-flex items-center justify-center gap-1.5 rounded-lg border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.18))] px-3 py-2 text-xs font-medium disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]';

export function AutoEqView({onOpenPreset}:{onOpenPreset?:()=>void}) {
 const {t}=useTranslation();
 const [catalog,setCatalog]=useState<EqCatalog|null>(null),[loading,setLoading]=useState(true),[working,setWorking]=useState(false),[error,setError]=useState(''),[showAdd,setShowAdd]=useState<boolean|null>(null),[category,setCategory]=useState<EquipmentCategory>('headphones');
 const alive=useRef(true),generation=useRef(0),busy=useRef(false);
 const load=useCallback(async()=>{const id=++generation.current;setLoading(true);try{const value=await equalizerService.get();if(alive.current&&generation.current===id){setCatalog(value);setError('')}}catch(reason){if(alive.current&&generation.current===id)setError(String(reason).startsWith('EQ_')?'unavailable':'unavailable')}finally{if(alive.current&&generation.current===id)setLoading(false)}},[]);
 useEffect(()=>{alive.current=true;void load();return()=>{alive.current=false;generation.current++}},[load]);
 const edit=async(action:EqEdit)=>{if(!catalog||busy.current)return false;busy.current=true;setWorking(true);setError('');const id=++generation.current;try{const value=await equalizerService.edit(catalog.revision,action);if(!alive.current||generation.current!==id)return false;setCatalog(value);return true}catch(reason){if(alive.current&&generation.current===id){setError(String(reason)==='EQ_STORE_CHANGED'?'changed':'saveFailed');if(String(reason)==='EQ_STORE_CHANGED')void load()}return false}finally{busy.current=false;if(alive.current)setWorking(false)}};
 const addManual=async(name:string,manufacturer:string)=>{const added=await edit({action:'add_equipment',id:null,name,category,manufacturer:manufacturer.trim()||null,opra_vendor_id:null,opra_product_id:null});if(added)setShowAdd(false);return added};
 const addOpra=async(product:OpraProductHit)=>{const added=await edit({action:'add_equipment',id:null,name:product.product_name,category:'headphones',manufacturer:product.vendor_name,opra_vendor_id:product.vendor_id,opra_product_id:product.product_id});if(added)setShowAdd(false);return added};
 const openPreset=async(reference:PresetRef)=>{if(await edit({action:'select',reference})){window.dispatchEvent(new Event('sonante:eq-preset-opened'));onOpenPreset?.()}};
 const equipment=catalog?.config.equipment_profiles??[];
 const addOpen=showAdd??equipment.length===0;
 const presets=[...(catalog?.built_ins??[]).map(preset=>({reference:{kind:'built_in' as const,id:preset.id},name:t(`equalizer.builtin.${preset.id}`)})),...(catalog?.config.user_presets??[]).map(preset=>({reference:{kind:'user' as const,id:preset.id},name:preset.name}))];
 return <main data-autoeq aria-labelledby="autoeq-title" className="min-w-0 min-h-0 flex-1 overflow-y-auto px-4 py-5 sm:px-6">
  <div className="mx-auto w-full max-w-6xl space-y-5">
   <header className="flex flex-wrap items-start gap-3"><h1 id="autoeq-title" className="flex items-center gap-2 text-xl font-semibold"><Sparkles className="text-[color:var(--theme-fg-accent,#E5A00D)]" aria-hidden="true"/>{t('autoeq.title')}</h1><p className="ml-auto max-w-xl text-xs text-[color:var(--theme-fg-muted,#999)]">{t('autoeq.readOnlyShort')}</p></header>
   {loading&&!catalog&&<p role="status" className="text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('autoeq.loading')}</p>}
   {error&&<p role="alert" className="rounded-lg border border-[color:var(--theme-border-warning,rgba(229,160,13,.35))] p-3 text-sm">{t(`autoeq.${error}`)}</p>}
   {catalog&&<>
    <MyEquipment equipment={equipment} presets={presets} busy={working} addOpen={addOpen} category={category} onToggleAdd={()=>setShowAdd(value=>!(value??equipment.length===0))} onCategory={setCategory} onManual={addManual} onEdit={edit} onOpenPreset={openPreset} onAddOpra={addOpra} t={t}/>
   </>}
  </div>
 </main>;
}

export function MyEquipment({equipment,presets,busy,addOpen,category,onToggleAdd,onCategory,onManual,onEdit,onOpenPreset,onAddOpra,t}:{equipment:EquipmentProfile[];presets:{reference:PresetRef;name:string}[];busy:boolean;addOpen:boolean;category:EquipmentCategory;onToggleAdd:()=>void;onCategory:(category:EquipmentCategory)=>void;onManual:(name:string,manufacturer:string)=>Promise<boolean>;onEdit:(action:EqEdit)=>Promise<boolean>;onOpenPreset:(reference:PresetRef)=>Promise<void>;onAddOpra:(product:OpraProductHit)=>Promise<boolean>;t:(key:string,options?:Record<string,unknown>)=>string}) {
 return <section aria-labelledby="my-equipment-title" data-my-equipment className="space-y-4">
  <div className="flex flex-wrap items-center gap-2"><Box className="text-[color:var(--theme-fg-accent,#E5A00D)]" size={19} aria-hidden="true"/><h2 id="my-equipment-title" className="text-lg font-semibold">{t('autoeq.myEquipment')}</h2>{equipment.length>0&&<span className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('autoeq.equipmentCount',{count:equipment.length})}</span>}<button type="button" aria-expanded={addOpen} aria-controls="add-equipment-panel" onClick={onToggleAdd} className={`${button} ml-auto`}><Plus size={14}/>{t('autoeq.addEquipment')}</button></div>
  {equipment.length===0&&!addOpen&&<p className="text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t('autoeq.emptyEquipment')}</p>}
  {addOpen&&<AddEquipmentPanel category={category} onCategory={onCategory} onManual={onManual} onAddOpra={onAddOpra} disabled={busy} registeredProductIds={equipment.flatMap(item=>item.opra_product_id?[item.opra_product_id]:[])} t={t}/>}
  {equipment.length===0&&!addOpen?null:<div className="grid gap-3 lg:grid-cols-2">{equipment.map(item=><EquipmentCard key={item.id} item={item} presets={presets} busy={busy} onEdit={onEdit} onOpenPreset={onOpenPreset} t={t}/>)}</div>}
 </section>;
}

export function AddEquipmentPanel({category,onCategory,onManual,onAddOpra,disabled,registeredProductIds,t}:{category:EquipmentCategory;onCategory:(category:EquipmentCategory)=>void;onManual:(name:string,manufacturer:string)=>Promise<boolean>;onAddOpra:(product:OpraProductHit)=>Promise<boolean>;disabled:boolean;registeredProductIds:string[];t:(key:string)=>string}) {
 const [name,setName]=useState(''),[manufacturer,setManufacturer]=useState('');
 const submit=async(event:FormEvent)=>{event.preventDefault();if(!name.trim()||disabled)return;if(await onManual(name.trim(),manufacturer.trim())){setName('');setManufacturer('')}};
 return <section id="add-equipment-panel" aria-label={t('autoeq.addEquipment')} className="space-y-4 rounded-xl border border-dashed border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.2))] p-4 sm:p-5">
  <label className="block max-w-sm text-xs">{t('autoeq.equipmentCategory')}<select value={category} disabled={disabled} onChange={event=>onCategory(event.target.value as EquipmentCategory)} className={input}><option value="headphones">{t('autoeq.category.headphones')}</option><option value="speakers">{t('autoeq.category.speakers')}</option><option value="other">{t('autoeq.category.other')}</option></select></label>
  <form onSubmit={submit} className="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
   <label className="text-xs">{t('autoeq.equipmentName')}<input required maxLength={80} value={name} disabled={disabled} onChange={event=>setName(event.target.value)} className={input}/></label>
   <label className="text-xs">{t('autoeq.manufacturerOptional')}<input maxLength={80} value={manufacturer} disabled={disabled} onChange={event=>setManufacturer(event.target.value)} className={input}/></label>
   <button type="submit" disabled={disabled||!name.trim()} className={`${button} bg-[var(--theme-control-accent,#E5A00D)] text-[color:var(--theme-control-on-accent,#111)]`}><Plus size={14}/>{t('autoeq.addManually')}</button>
  </form>
  {category==='headphones'&&<div className="border-t border-white/10 pt-3"><p className="mb-3 text-sm font-medium">{t('autoeq.searchOpraHeadphones')}</p><OpraCatalogBrowser disabled={disabled} registeredProductIds={registeredProductIds} onAddProduct={onAddOpra}/></div>}
 </section>;
}

function EquipmentCard({item,presets,busy,onEdit,onOpenPreset,t}:{item:EquipmentProfile;presets:{reference:PresetRef;name:string}[];busy:boolean;onEdit:(action:EqEdit)=>Promise<boolean>;onOpenPreset:(reference:PresetRef)=>Promise<void>;t:(key:string,options?:Record<string,unknown>)=>string}) {
 const [selected,setSelected]=useState(''),preferred=item.preferred_preset?refKey(item.preferred_preset):'';
 const CategoryIcon=item.category==='headphones'?Headphones:item.category==='speakers'?Speaker:Box;
 const findName=(reference:PresetRef)=>presets.find(preset=>refKey(preset.reference)===refKey(reference))?.name??t('autoeq.missingPreset');
 return <article data-equipment-id={item.id} className="space-y-3 rounded-xl border border-[color:var(--theme-border-soft,rgb(255_255_255_/_0.14))] bg-[color:var(--theme-surface-panel,rgba(255,255,255,.02))] p-4">
  <header className="flex min-w-0 items-start gap-3"><span className="rounded-lg bg-[color:var(--theme-control-accent,rgba(229,160,13,.12))] p-2 text-[color:var(--theme-fg-accent,#E5A00D)]"><CategoryIcon size={18}/></span><div className="min-w-0 flex-1"><p className="text-[10px] font-semibold uppercase tracking-wide text-[color:var(--theme-fg-muted,#999)]">{t(`autoeq.category.${item.category}`)}</p><h3 className="truncate font-semibold">{item.name}</h3><p className="text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{item.manufacturer||t('autoeq.manufacturerNotSet')}{item.opra_product_id&&<span className="ml-2 rounded-full border border-white/10 px-2 py-0.5">{t('autoeq.catalogSource')}</span>}</p></div><button type="button" disabled={busy} aria-label={t('autoeq.removeEquipment')} onClick={()=>void onEdit({action:'unlink_equipment',id:item.id})} className="rounded p-2 text-[color:var(--theme-fg-muted,#999)] hover:text-[color:var(--theme-fg-error,#f88)]"><Trash2 size={15}/></button></header>
  <details className="group rounded-lg border border-white/10 p-3" open={item.presets.length>0}><summary className="cursor-pointer list-none text-sm font-medium">{t('autoeq.presetManagement')} <span className="text-xs text-[color:var(--theme-fg-muted,#999)]">({item.presets.length})</span></summary>
   <div className="mt-3 space-y-3"><div className="space-y-2">{item.presets.length===0?<p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('autoeq.noPresets')}</p>:<ul className="space-y-1">{item.presets.map(reference=><li key={refKey(reference)} className="flex flex-wrap items-center gap-2 text-xs"><button type="button" disabled={busy||preferred===refKey(reference)} aria-pressed={preferred===refKey(reference)} onClick={()=>void onEdit({action:'set_preferred_equipment_preset',id:item.id,reference})} className="rounded px-2 py-1 text-left hover:bg-white/5 disabled:opacity-60">{preferred===refKey(reference)?'★':'☆'} {findName(reference)}{preferred===refKey(reference)&&<span className="ml-1 text-[color:var(--theme-fg-muted,#999)]">{t('autoeq.preferred')}</span>}</button><button type="button" disabled={busy} aria-label={t('autoeq.removeAssociation')} onClick={()=>void onEdit({action:'remove_equipment_preset',id:item.id,reference})} className="rounded p-1 text-[color:var(--theme-fg-muted,#999)] hover:text-[color:var(--theme-fg-error,#f88)]"><Trash2 size={13}/></button></li>)}</ul>}</div>
   <div className="grid gap-2 sm:grid-cols-[1fr_auto] sm:items-end"><label className="text-xs">{t('autoeq.addSavedPreset')}<select aria-label={t('autoeq.addSavedPreset')} value={selected} disabled={busy||presets.length===0} onChange={event=>setSelected(event.target.value)} className={input}><option value="">{t('autoeq.choosePreset')}</option>{presets.map(preset=><option key={refKey(preset.reference)} value={refKey(preset.reference)}>{preset.name}</option>)}</select></label><button type="button" disabled={busy||!selected||item.presets.some(reference=>refKey(reference)===selected)} onClick={()=>{const reference=presets.find(preset=>refKey(preset.reference)===selected)?.reference;if(reference){void onEdit({action:'add_equipment_preset',id:item.id,reference,preferred:item.presets.length===0})}}} className={button}><Plus size={13}/>{t('autoeq.associatePreset')}</button></div>
   {item.preferred_preset&&<button type="button" disabled={busy} onClick={()=>void onOpenPreset(item.preferred_preset!)} className={button}>{t('autoeq.openPreferred')}</button>}</div>
  </details>
 </article>;
}
