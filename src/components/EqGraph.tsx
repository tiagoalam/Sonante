import {useMemo,useRef} from 'react';
import {useTranslation} from 'react-i18next';
import type {EqBand,EqPreset,Protection} from '../types/equalizer';
import {BAND_COLORS,frequencyX,xFrequency,clampGain,movePoint,responseDb,graphPoint} from '../utils/equalizer';
const X=48,W=904,Y=28,H=268;
export function EqGraph({preset,budget,selected,onSelect,onChange,onAdd,showGain=false,onToggleGain,disabled=false}:{preset:EqPreset;budget:Protection|null;selected:string|null;onSelect:(id:string|null)=>void;onChange:(id:string,patch:Partial<EqBand>)=>void;onAdd:(hz:number,gain:number)=>void;showGain?:boolean;onToggleGain?:()=>void;disabled?:boolean}){
 const{t}=useTranslation();const drag=useRef<{id:string;before:EqBand}|null>(null);
 const hz=useMemo(()=>Array.from({length:241},(_,i)=>xFrequency(i/240)),[]);
 // Auto-expand only for curves that would leave the default ±15 dB view.
 // The same range drives labels, all paths, points and pointer→gain conversion.
 const samples=useMemo(()=>({individual:preset.bands.map(b=>b.enabled?hz.map(f=>responseDb({...preset,preamp_db:0,bands:[b]},f)):null),requested:hz.map(f=>responseDb({...preset,preamp_db:0},f)),combined:budget?hz.map(f=>responseDb({...preset,preamp_db:budget.effective_preamp_db},f)):null}),[preset,budget,hz]);
 const extent=Math.max(15,...samples.requested.map(Math.abs),...samples.individual.flatMap(v=>v?.map(Math.abs)??[]),...(showGain?samples.combined?.map(Math.abs)??[]:[]));
 const range=15*Math.ceil(extent/15);
 const y=(db:number)=>Y+H/2-db*H/(2*range);
 const yGain=(at:number)=>clampGain((.5-(at-Y)/H)*2*range);
 const path=(v:number[])=>v.map((db,i)=>`${i?'L':'M'}${X+i*W/240},${y(db)}`).join(' ');
 const curves={individual:samples.individual.map(v=>v?path(v):null),requested:path(samples.requested),combined:samples.combined?path(samples.combined):null};
 const pos=(e:React.PointerEvent<SVGSVGElement>|React.MouseEvent<SVGSVGElement>)=>{return graphPoint(e.clientX,e.clientY,e.currentTarget.getBoundingClientRect())};
 return <figure data-theme-surface="instrument" className="rounded-xl border border-white/10 bg-[#111214] p-3" aria-label={t('equalizer.curve')}>
  <svg id="equalizer-graph" tabIndex={0} preserveAspectRatio="xMidYMid meet" role="group" aria-label={t('equalizer.graphHelp')} viewBox="0 0 1000 350" className="w-full min-h-56 max-h-96" style={{touchAction:'pan-y'}}
   onDoubleClick={e=>{if(disabled||drag.current||e.target!==e.currentTarget&&((e.target as Element).tagName==='circle'))return;const p=pos(e);if(p&&p.x>=X&&p.x<=X+W&&p.y>=Y&&p.y<=Y+H)onAdd(xFrequency((p.x-X)/W),yGain(p.y))}}
   onPointerMove={e=>{if(!drag.current||disabled)return;const p=pos(e);if(!p)return;onChange(drag.current.id,{frequency_hz:xFrequency((p.x-X)/W),gain_db:yGain(p.y)})}}
   onPointerUp={()=>{drag.current=null}} onPointerCancel={()=>{if(drag.current)onChange(drag.current.id,drag.current.before);drag.current=null}}
   onKeyDown={e=>{if(e.key==='Escape'){if(drag.current)onChange(drag.current.id,drag.current.before);drag.current=null;onSelect(null);}}}>
   {[-range,-range*2/3,-range/3,0,range/3,range*2/3,range].map(db=><g key={db}><line x1={X} x2={X+W} y1={y(db)} y2={y(db)} stroke={db===0?'#62676e':'#292c30'}/><text x="2" y={y(db)+4} fontSize="12" fill="#92979f">{db}{t('equalizer.dbUnit')}</text></g>)}
   {[20,50,100,200,500,1000,2000,5000,10000,20000].map(f=><g key={f}><line x1={X+frequencyX(f)*W} x2={X+frequencyX(f)*W} y1={Y} y2={Y+H} stroke="#25292c"/><text x={X+frequencyX(f)*W} y="320" textAnchor="middle" fill="#92979f" fontSize="12">{f>=1000?`${f/1000}k`:f}</text></g>)}
   {curves.individual.map((p,i)=>p&&<path key={preset.bands[i].id} d={p} fill="none" stroke={BAND_COLORS[i]} opacity=".6" strokeWidth="1.2" pointerEvents="none"/>)}
   <path d={curves.requested} fill="none" stroke="#E5A00D" strokeWidth="2.8" pointerEvents="none"/>
   {showGain&&curves.combined&&<path d={curves.combined} fill="none" stroke="#a0a4ad" strokeWidth="1.8" strokeDasharray="5 5" pointerEvents="none"/>}
   {preset.bands.map((b,i)=>b.enabled&&<circle key={b.id} role="button" tabIndex={disabled?-1:0} aria-disabled={disabled} aria-pressed={selected===b.id} aria-label={`${t('equalizer.band')} ${i+1}: ${Math.round(b.frequency_hz)} ${t('equalizer.hzUnit')}, ${b.gain_db.toFixed(1)} ${t('equalizer.dbUnit')}`} cx={X+frequencyX(b.frequency_hz)*W} cy={y(b.gain_db)} r={selected===b.id?8:6} fill={BAND_COLORS[i]} stroke={selected===b.id?'#fff':'#c4c4c4'} strokeWidth="1.5" style={{cursor:'grab',touchAction:'none'}}
    onPointerDown={e=>{if(disabled||e.button!==0)return;e.preventDefault();onSelect(b.id);drag.current={id:b.id,before:{...b}};e.currentTarget.setPointerCapture(e.pointerId);e.currentTarget.focus()}}
    onClick={()=>onSelect(b.id)} onKeyDown={e=>{if(disabled)return;if(e.key==='Enter'||e.key===' '){e.preventDefault();onSelect(b.id)}else if(e.key==='Delete'||e.key==='Backspace'){e.preventDefault();onChange(b.id,{enabled:false});onSelect(null);e.currentTarget.ownerSVGElement?.focus()}else{const patch=movePoint(b,e.key,e.shiftKey);if(patch){e.preventDefault();onSelect(b.id);onChange(b.id,patch)}}}}/>)}
  </svg>
  <label className="my-2 flex items-center gap-2 text-xs text-white/65"><input type="checkbox" aria-label={t('equalizer.showGlobalGain')} checked={showGain} disabled={disabled||!onToggleGain} onChange={onToggleGain}/>{t('equalizer.showGlobalGain')}</label>
  <figcaption className="space-y-1 text-[11px] leading-relaxed text-white/50"><p>{t('equalizer.graphScale',{range})}{range>15?' · '+t('equalizer.graphExpanded'):''}</p><p>{t('equalizer.graphLegend')}</p><p>{t('equalizer.graphHelp')}</p></figcaption>
 </figure>;
}
