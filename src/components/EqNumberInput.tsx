import {useEffect,useRef,useState} from 'react';
import {formatEqNumber,parseEqNumber} from './eqNumbers';
/** Keep an unrounded model and a transient editing string, like a native number
 * input. Incomplete/invalid text publishes NaN so existing draft validation
 * blocks Apply; no audio or persistence work happens here. */
export function EqNumberInput({value,onValueChange,locale='en',digits=2,min,max,step,disabled=false,className,...label}:{value:number;onValueChange:(value:number)=>void;locale?:string;digits?:number;min:number;max:number;step:number;disabled?:boolean;className?:string;'aria-label':string}) {
 const [text,setText]=useState(()=>formatEqNumber(value,digits,locale));
 const focused=useRef(false),edited=useRef(false),origin=useRef(value),emitted=useRef<number|undefined>(undefined),language=useRef(locale);
 useEffect(()=>{
  const translated=language.current!==locale;language.current=locale;
  if(focused.current&&edited.current&&Object.is(value,emitted.current)) {
   if(translated&&Number.isFinite(value))setText(formatEqNumber(value,digits,locale));
   return;
  }
  setText(formatEqNumber(value,digits,locale));edited.current=false;origin.current=value;
 },[value,locale,digits]);
 const publish=(n:number)=>{edited.current=true;emitted.current=n;onValueChange(n)};
 const invalid=!Number.isFinite(value)||value<min||value>max;
 return <input {...label} type="text" inputMode="decimal" role="spinbutton" min={min} max={max} step={step} aria-valuemin={min} aria-valuemax={max} aria-valuenow={Number.isFinite(value)?value:undefined} aria-valuetext={formatEqNumber(value,digits,locale)} aria-invalid={invalid||undefined} disabled={disabled} className={className} value={text}
  onFocus={()=>{focused.current=true;edited.current=false;origin.current=value;emitted.current=undefined}}
  onChange={e=>{const next=e.target.value;setText(next);publish(parseEqNumber(next))}}
  onBlur={()=>{focused.current=false;if(Number.isFinite(value))setText(formatEqNumber(value,digits,locale))}}
  onKeyDown={e=>{
   if(e.key==='Escape'&&edited.current){e.preventDefault();e.stopPropagation();const previous=origin.current;edited.current=false;emitted.current=previous;setText(formatEqNumber(previous,digits,locale));if(!Object.is(value,previous))onValueChange(previous)}
   else if(e.key==='Enter'){e.preventDefault();e.stopPropagation();e.currentTarget.blur()}
   else if(e.key==='ArrowUp'||e.key==='ArrowDown'){e.preventDefault();e.stopPropagation();const next=Number.isFinite(value)?Math.min(max,Math.max(min,value+(e.key==='ArrowUp'?step:-step))):Math.min(max,Math.max(min,0));setText(formatEqNumber(next,digits,locale));publish(next)}
  }}/>
}
