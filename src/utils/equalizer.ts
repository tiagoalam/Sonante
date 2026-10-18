import type { EqBand, EqPreset } from "../types/equalizer";
export function flatPreset(): EqPreset {
  return { id: "flat", name: "Flat", preamp_db: 0, bands: [31,63,125,250,500,1000,2000,4000,8000,16000].map((frequency_hz,i)=>({id:`flat_${String(i+1).padStart(2,"0")}`,enabled:false,filter_type:"peak",frequency_hz,gain_db:0,q:1})) };
}
// RBJ biquads, matching Camilla's Peaking/Lowshelf/Highshelf Q definition.
export function coefficients(b: EqBand, rate: number) {
  const A=10**(b.gain_db/40),w=2*Math.PI*b.frequency_hz/rate,c=Math.cos(w),alpha=Math.sin(w)/(2*b.q),s=2*Math.sqrt(A)*alpha;
  if(b.filter_type==="low_shelf") return [A*((A+1)-(A-1)*c+s),2*A*((A-1)-(A+1)*c),A*((A+1)-(A-1)*c-s),(A+1)+(A-1)*c+s,-2*((A-1)+(A+1)*c),(A+1)+(A-1)*c-s];
  if(b.filter_type==="high_shelf") return [A*((A+1)+(A-1)*c+s),-2*A*((A-1)+(A+1)*c),A*((A+1)+(A-1)*c-s),(A+1)-(A-1)*c+s,2*((A-1)-(A+1)*c),(A+1)-(A-1)*c-s];
  return [1+alpha*A,-2*c,1-alpha*A,1+alpha/A,-2*c,1-alpha/A];
}
export function responseDb(preset: EqPreset, frequency: number, rate=48000): number {
  const w=2*Math.PI*frequency/rate,c=Math.cos(w),s=Math.sin(w),c2=Math.cos(2*w),s2=Math.sin(2*w);
  let value=preset.preamp_db;
  for(const band of preset.bands){if(!band.enabled)continue;const[b0,b1,b2,a0,a1,a2]=coefficients(band,rate);const n=(b0+b1*c+b2*c2)**2+(b1*s+b2*s2)**2,d=(a0+a1*c+a2*c2)**2+(a1*s+a2*s2)**2;value+=10*Math.log10(Math.max(n,1e-30)/Math.max(d,1e-30));}
  return value;
}
export function sameSettings(a: EqPreset | null | undefined,b: EqPreset): boolean {
  return !!a && a.preamp_db===b.preamp_db && a.bands.length===b.bands.length && a.bands.every((v,i)=>{const t=b.bands[i];return v.enabled===t.enabled&&v.filter_type===t.filter_type&&v.frequency_hz===t.frequency_hz&&v.gain_db===t.gain_db&&v.q===t.q;});
}
export function validDraft(p: EqPreset): boolean {
  if(!p||typeof p.id!=="string"||!Array.isArray(p.bands)||p.bands.some(b=>!b||typeof b.id!=="string"))return false;
  return /^[A-Za-z0-9_-]{1,96}$/.test(p.id)&&typeof p.name==="string"&&p.name.trim().length>0&&Array.from(p.name.trim()).length<=80&&!/[\x00-\x1f\x7f-\x9f]/.test(p.name)&&new Set(p.bands.map(b=>b.id)).size===10&&p.bands.every(b=>/^[A-Za-z0-9_-]{1,96}$/.test(b.id)&&typeof b.enabled==="boolean"&&["peak","low_shelf","high_shelf"].includes(b.filter_type))&&p.bands.length===10&&Number.isFinite(p.preamp_db)&&p.preamp_db>=-24&&p.preamp_db<=12&&p.bands.every(b=>Number.isFinite(b.frequency_hz)&&b.frequency_hz>=20&&b.frequency_hz<=20000&&Number.isFinite(b.gain_db)&&b.gain_db>=-12&&b.gain_db<=12&&Number.isFinite(b.q)&&b.q>=.1&&b.q<=30);
}

export const BAND_COLORS=["#91a7ff","#66d9c5","#e8bb72","#f28ca9","#aedb77","#a6c8ef","#c7a5ed","#78c4c9","#dfb5a0","#e3dc80"];
export const frequencyX=(hz:number)=>Math.log(Math.max(20,Math.min(20000,hz))/20)/Math.log(1000);
export const xFrequency=(x:number)=>20*1000**Math.max(0,Math.min(1,x));
export const clampGain=(gain:number)=>Math.max(-12,Math.min(12,gain));
export function movePoint(b:EqBand,key:string,shift=false):Partial<EqBand>|null {
 const scale=shift?1.002:1.025,step=shift?.1:.5;
 if(key==='ArrowLeft'||key==='ArrowRight')return{frequency_hz:Math.max(20,Math.min(20000,b.frequency_hz*(key==='ArrowLeft'?1/scale:scale)))};
 if(key==='ArrowUp'||key==='ArrowDown')return{gain_db:clampGain(b.gain_db+(key==='ArrowUp'?step:-step))};
 return null;
}
// Same prefix + analytic geometric tail budget as the backend. Backend is authoritative.
export function protection(p:EqPreset,rate=48000):import('../types/equalizer').Protection|null {
 if(!validDraft(p))return null;let db=0;
 for(const b of p.bands){if(!b.enabled||b.gain_db===0)continue;
  const[b0,b1,b2,a0,a1raw,a2raw]=coefficients(b,rate),a1=a1raw/a0,a2=a2raw/a0;
  const B0=b0/a0,B1=b1/a0,B2=b2/a0,disc=a1*a1-4*a2,gap=Math.sqrt(Math.abs(disc));const r=disc<0?Math.sqrt(a2):Math.max(Math.abs((-a1+gap)/2),Math.abs((-a1-gap)/2));
  if(!Number.isFinite(r)||r>=1||gap<1e-10)return null;
  let prev=B0,curr=B1-a1*B0,sum=Math.abs(B0)+Math.abs(curr),next=B2-a1*curr-a2*prev;prev=curr;curr=next;sum+=Math.abs(curr);
  for(let i=3;i<4096;i++){next=-a1*curr-a2*prev;prev=curr;curr=next;sum+=Math.abs(curr);}
  const t1=-a1*curr-a2*prev,t2=-a1*t1-a2*curr,tail=disc<0?Math.sqrt(t1*t1+((t2+a1*t1/2)/(gap/2))**2)/(1-r):Math.abs((t2-((-a1-gap)/2)*t1)/gap)/(1-Math.abs((-a1+gap)/2))+Math.abs((((-a1+gap)/2)*t1-t2)/gap)/(1-Math.abs((-a1-gap)/2));
  db+=20*Math.log10(Math.max(1,(sum+Math.abs(t1)+tail)*(1+1e-8)));
 }
 const resampler=20*Math.log10(4),effective=Math.min(p.preamp_db,0)-1-db-resampler,attenuation=p.preamp_db-effective;
 if(!Number.isFinite(attenuation)||attenuation>48)return null;
 return{requested_preamp_db:p.preamp_db,effective_preamp_db:effective,attenuation_db:attenuation,filter_bound_db:db,resampler_bound_db:resampler,ceiling_db:-1};
}

export function graphPoint(clientX:number,clientY:number,r:{left:number;top:number;width:number;height:number}){
 const scale=Math.min(r.width/1000,r.height/350);if(!Number.isFinite(scale)||scale<=0)return null;
 return{x:(clientX-r.left-(r.width-1000*scale)/2)/scale,y:(clientY-r.top-(r.height-350*scale)/2)/scale};
}
