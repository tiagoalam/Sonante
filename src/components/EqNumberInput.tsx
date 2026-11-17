import {useEffect,useRef,useState} from 'react';
import {formatEqNumber,parseEqNumber} from './eqNumbers';
/** Keep an unrounded valid model and a transient editing string, like a native number input. */
export function EqNumberInput({value,onValueChange,onValidityChange,locale='en',digits=2,min,max,step,disabled=false,className,...label}:{value:number;onValueChange:(value:number)=>void;onValidityChange?:(invalid:boolean)=>void;locale?:string;digits?:number;min:number;max:number;step:number;disabled?:boolean;className?:string;'aria-label':string;'aria-describedby'?:string}) {
 const [text,setText]=useState(()=>formatEqNumber(value,digits,locale));
 const focused=useRef(false),edited=useRef(false),origin=useRef(value),emitted=useRef(value),language=useRef(locale),reportedInvalid=useRef(false);
 const reportValidity=(invalid:boolean)=>{if(reportedInvalid.current===invalid)return;reportedInvalid.current=invalid;onValidityChange?.(invalid)};
 useEffect(()=>{
  const translated=language.current!==locale;language.current=locale;
  if(edited.current&&Object.is(value,emitted.current)) {
   if(translated&&Number.isFinite(value)&&Number.isFinite(parseEqNumber(text))&&parseEqNumber(text)>=min&&parseEqNumber(text)<=max)setText(formatEqNumber(value,digits,locale));
   return;
  }
  setText(formatEqNumber(value,digits,locale));edited.current=false;origin.current=value;emitted.current=value;
  const invalid=!Number.isFinite(value)||value<min||value>max;
  if(reportedInvalid.current!==invalid){reportedInvalid.current=invalid;onValidityChange?.(invalid)}
 },[value,locale,digits,min,max]);
 const publish=(n:number)=>{edited.current=true;emitted.current=n;reportValidity(false);onValueChange(n)};
 const parsed=parseEqNumber(text),invalid=!Number.isFinite(parsed)||parsed<min||parsed>max||!Number.isFinite(value)||value<min||value>max;
 return <input {...label} type="text" inputMode="decimal" role="spinbutton" min={min} max={max} step={step} aria-valuemin={min} aria-valuemax={max} aria-valuenow={!invalid&&Number.isFinite(value)?value:undefined} aria-valuetext={invalid?text:formatEqNumber(value,digits,locale)} aria-invalid={invalid||undefined} disabled={disabled} className={className} value={text}
  onFocus={()=>{focused.current=true;edited.current=false;origin.current=value;emitted.current=value}}
  onChange={e=>{const next=e.target.value,n=parseEqNumber(next),nextInvalid=!Number.isFinite(n)||n<min||n>max;setText(next);edited.current=true;reportValidity(nextInvalid);if(!nextInvalid){emitted.current=n;onValueChange(n)}}}
  onBlur={e=>{focused.current=false;const next=parseEqNumber(e.currentTarget.value);if(Number.isFinite(next)&&next>=min&&next<=max){const formatted=formatEqNumber(next,digits,locale);setText(formatted);edited.current=false;origin.current=next;emitted.current=next;reportValidity(false)}else reportValidity(true)}}
  onKeyDown={e=>{
   if(e.key==='Escape'&&edited.current){e.preventDefault();e.stopPropagation();const previous=origin.current;edited.current=false;emitted.current=previous;setText(formatEqNumber(previous,digits,locale));reportValidity(false);if(!Object.is(value,previous))onValueChange(previous)}
   else if(e.key==='Enter'){e.preventDefault();e.stopPropagation();e.currentTarget.blur()}
   else if(e.key==='ArrowUp'||e.key==='ArrowDown'){e.preventDefault();e.stopPropagation();const next=Number.isFinite(value)?Math.min(max,Math.max(min,value+(e.key==='ArrowUp'?step:-step))):Math.min(max,Math.max(min,0));setText(formatEqNumber(next,digits,locale));publish(next)}
  }}/>;
}
