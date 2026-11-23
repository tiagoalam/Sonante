import {useEffect,useMemo,useRef,useState} from 'react';
import {useTranslation} from 'react-i18next';
import type {EqBand,EqPreset,Protection} from '../types/equalizer';
import {formatEqNumber} from './eqNumbers';
import {EqHelp} from './EqHelp';
import {BAND_COLORS,frequencyX,xFrequency,movePoint,graphPoint} from '../utils/equalizer';
import {equalizerGraphFrequencyLabels,equalizerGraphGain,equalizerGraphPath,equalizerGraphRange,equalizerGraphTicks,equalizerGraphY,sampleEqualizerGraph} from '../utils/equalizerGraph';
const X=48,Y=28,H=268;
export function EqGraph({preset,budget,selected,onSelect,onChange,onAdd,showGain=false,onToggleGain,disabled=false,caption}:{preset:EqPreset;budget:Protection|null;selected:string|null;onSelect:(id:string|null)=>void;onChange:(id:string,patch:Partial<EqBand>)=>void;onAdd:(hz:number,gain:number)=>void;showGain?:boolean;onToggleGain?:()=>void;disabled?:boolean;caption?:string}){
 const{t,i18n}=useTranslation();const language=i18n?.resolvedLanguage??i18n?.language??'en';const graph=useRef<SVGSVGElement>(null),[viewWidth,setViewWidth]=useState(1000);
 useEffect(()=>{const svg=graph.current;if(!svg||typeof ResizeObserver==='undefined')return;const observer=new ResizeObserver(([entry])=>{const {width,height}=entry.contentRect;if(width>0&&height>0)setViewWidth(Math.max(320,350*width/height))});observer.observe(svg);return()=>observer.disconnect()},[]);
 const W=viewWidth-96;const drag=useRef<{id:string;before:EqBand}|null>(null);
 const samples=useMemo(()=>sampleEqualizerGraph(preset,budget?.effective_preamp_db??null,showGain),[preset,budget?.effective_preamp_db,showGain]);
 const scaleCurves=[...samples.individual,samples.requested,...(samples.combined?[samples.combined]:[]),preset.bands.filter(b=>b.enabled).map(b=>b.gain_db)];
 const range=equalizerGraphRange(scaleCurves);
 const dBLabels=equalizerGraphTicks(range),frequencyLabels=equalizerGraphFrequencyLabels(W);
 const y=(db:number)=>equalizerGraphY(db,Y,H,range);
 const yGain=(at:number)=>equalizerGraphGain(at,Y,H,range);
 const path=(v:number[])=>equalizerGraphPath(v,X,W,Y,H,range);
 const curves={individual:samples.individual.map(v=>v?path(v):null),requested:path(samples.requested),combined:samples.combined?path(samples.combined):null};
 const pos=(e:React.PointerEvent<SVGSVGElement>|React.MouseEvent<SVGSVGElement>)=>{const svg=e.currentTarget,matrix=svg.getScreenCTM?.();if(matrix){const p=svg.createSVGPoint();p.x=e.clientX;p.y=e.clientY;return p.matrixTransform(matrix.inverse())}return graphPoint(e.clientX,e.clientY,svg.getBoundingClientRect())};
 return <figure data-theme-surface="instrument" className="eq-graph rounded-xl border border-white/10 bg-[#111214]" aria-label={t('equalizer.curve')}>
  <svg ref={graph} id="equalizer-graph" tabIndex={0} preserveAspectRatio="xMidYMid meet" role="group" aria-label={t('equalizer.graphHelp')} viewBox={`0 0 ${viewWidth} 350`} className="w-full" style={{touchAction:'pan-y'}}
   onDoubleClick={e=>{if(disabled||drag.current||e.target!==e.currentTarget&&((e.target as Element).tagName==='circle'))return;const p=pos(e);if(p&&p.x>=X&&p.x<=X+W&&p.y>=Y&&p.y<=Y+H)onAdd(xFrequency((p.x-X)/W),yGain(p.y))}}
   onPointerMove={e=>{if(!drag.current||disabled)return;const p=pos(e);if(!p)return;onChange(drag.current.id,{frequency_hz:xFrequency((p.x-X)/W),gain_db:yGain(p.y)})}}
   onPointerUp={()=>{drag.current=null}} onPointerCancel={()=>{if(drag.current)onChange(drag.current.id,drag.current.before);drag.current=null}}
   onKeyDown={e=>{if(e.key==='Escape'){if(drag.current)onChange(drag.current.id,drag.current.before);drag.current=null;onSelect(null);}}}>
   <defs><clipPath id="equalizer-curve-clip"><rect x={X} y={Y} width={W} height={H}/></clipPath></defs>
   {dBLabels.map(db=><g key={db}><line x1={X} x2={X+W} y1={y(db)} y2={y(db)} stroke={db===0?'#939ba6':'#3a3f47'} strokeWidth={db===0?1.8:1}/><text x={X-7} y={y(db)+4} textAnchor="end" fontSize="12" fill="#c0c5cd">{formatEqNumber(db,1,language)}{t('equalizer.dbUnit')}</text></g>)}
   {[20,50,100,200,500,1000,2000,5000,10000,20000].map(f=><g key={f}><line x1={X+frequencyX(f)*W} x2={X+frequencyX(f)*W} y1={Y} y2={Y+H} stroke="#30353d"/>{frequencyLabels.includes(f)&&<text x={X+frequencyX(f)*W} y="320" textAnchor={f===20?'start':f===20000?'end':'middle'} fill="#c0c5cd" fontSize={W<460?10:12}>{f>=1000?`${f/1000}k`:f}</text>}</g>)}
   <g clipPath="url(#equalizer-curve-clip)">
    {curves.individual.map((p,i)=>p&&<path key={preset.bands[i].id} d={p} fill="none" stroke={BAND_COLORS[i]} opacity=".96" strokeWidth="2.2" strokeLinejoin="round" strokeLinecap="round" pointerEvents="none"/>)}
    <path d={curves.requested} fill="none" stroke="#f0b429" strokeWidth="3.2" strokeLinejoin="round" strokeLinecap="round" pointerEvents="none"/>
    {showGain&&curves.combined&&<path d={curves.combined} fill="none" stroke="#e0e4eb" opacity=".98" strokeWidth="2.6" strokeDasharray="7 5" strokeLinejoin="round" strokeLinecap="round" pointerEvents="none"/>}
   </g>
   {preset.bands.map((b,i)=>b.enabled&&<g key={b.id}><circle data-eq-point={b.id} role="button" tabIndex={disabled?-1:0} aria-disabled={disabled} aria-pressed={selected===b.id} aria-label={`${t('equalizer.band')} ${i+1}: ${formatEqNumber(b.frequency_hz,0,language)} ${t('equalizer.hzUnit')}, ${formatEqNumber(b.gain_db,2,language)} ${t('equalizer.dbUnit')}`} cx={X+frequencyX(b.frequency_hz)*W} cy={y(b.gain_db)} r={selected===b.id?11:9} fill={BAND_COLORS[i]} stroke={selected===b.id?'#ffffff':'#202226'} strokeWidth={selected===b.id?3.5:2} className="eq-graph-point" style={{cursor:'grab',touchAction:'none'}}
    onPointerDown={e=>{if(disabled||e.button!==0)return;e.preventDefault();onSelect(b.id);drag.current={id:b.id,before:{...b}};e.currentTarget.setPointerCapture(e.pointerId);e.currentTarget.focus()}}
    onClick={()=>onSelect(b.id)} onKeyDown={e=>{if(disabled)return;if(e.key==='Enter'||e.key===' '){e.preventDefault();onSelect(b.id)}else if(e.key==='Delete'||e.key==='Backspace'){e.preventDefault();onChange(b.id,{enabled:false});onSelect(null);e.currentTarget.ownerSVGElement?.focus()}else{const patch=movePoint(b,e.key,e.shiftKey);if(patch){e.preventDefault();onSelect(b.id);onChange(b.id,patch)}}}}/></g>)}
   {preset.bands.map((b,i)=>b.enabled&&b.id===selected&&<text key={b.id} data-eq-point-number={b.id} x={X+frequencyX(b.frequency_hz)*W} y={y(b.gain_db)+4.5} textAnchor="middle" fontSize="13" fontWeight="700" fill="#111214" pointerEvents="none" aria-hidden="true">{i+1}</text>)}
  </svg>
  <figcaption className="eq-graph-caption">
   <div className="eq-graph-options"><label className="flex items-center gap-2 text-xs text-white/65"><input type="checkbox" aria-label={t('equalizer.showGlobalGain')} checked={showGain} disabled={disabled||!onToggleGain} onChange={onToggleGain}/>{t('equalizer.showGlobalGain')}</label><p>{t('equalizer.graphScale',{range})}{range>15?' · '+t('equalizer.graphExpanded'):''}</p><EqHelp title={t('equalizer.graphHelpTitle')}><p>{t('equalizer.graphLegend')}</p><p>{t('equalizer.graphHelp')}</p><p>{t('equalizer.removeHelp')}</p></EqHelp></div>
   <p className="eq-curve-label">{caption??t('equalizer.draftCurve')}</p>
  </figcaption>
 </figure>;
}
