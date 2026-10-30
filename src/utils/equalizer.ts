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
// Prefix + analytic infinite-tail bound, matching the authoritative backend.
function impulseBound(b:EqBand,rate:number):number|null {
 if(!b.enabled||b.gain_db===0)return 1;
 const[b0,b1,b2,a0,a1raw,a2raw]=coefficients(b,rate),a1=a1raw/a0,a2=a2raw/a0;
 const B0=b0/a0,B1=b1/a0,B2=b2/a0,disc=a1*a1-4*a2,gap=Math.sqrt(Math.abs(disc)),r=disc<0?Math.sqrt(a2):Math.max(Math.abs((-a1+gap)/2),Math.abs((-a1-gap)/2));
 if(!Number.isFinite(r)||r>=1||gap<1e-10)return null;
 let prev=B0,curr=B1-a1*B0,sum=Math.abs(B0)+Math.abs(curr),next=B2-a1*curr-a2*prev;prev=curr;curr=next;sum+=Math.abs(curr);
 for(let i=3;i<4096;i++){next=-a1*curr-a2*prev;prev=curr;curr=next;sum+=Math.abs(curr);}
 const t1=-a1*curr-a2*prev,t2=-a1*t1-a2*curr,tail=disc<0?Math.sqrt(t1*t1+((t2+a1*t1/2)/(gap/2))**2)/(1-r):Math.abs((t2-((-a1-gap)/2)*t1)/gap)/(1-Math.abs((-a1+gap)/2))+Math.abs((((-a1+gap)/2)*t1-t2)/gap)/(1-Math.abs((-a1-gap)/2));
 const bound=(sum+Math.abs(t1)+tail)*(1+1e-8);return Number.isFinite(bound)?Math.max(1,bound):null;
}
function cascadeBound(p:EqPreset,rate:number,product:number):number {
 if(rate!==48000)return product;const bands=p.bands.filter(b=>b.enabled&&b.gain_db!==0);if(!bands.length)return 1;
 const n=16384,k=Math.floor(n/bands.length),h=new Float64Array(n);h[0]=1;let tails=0,numerical=0,numericInput=1;const gamma=64*Number.EPSILON/(1-64*Number.EPSILON);
 for(const band of bands){
  const[B0,B1,B2,A0,A1,A2]=coefficients(band,rate),b0=B0/A0,b1=B1/A0,b2=B2/A0,a1=A1/A0,a2=A2/A0,l=impulseBound(band,rate);if(l===null)return product;
  const d=a1*a1-4*a2,gap=Math.sqrt(Math.abs(d)),r=d<0?Math.sqrt(a2):Math.max(Math.abs((-a1+gap)/2),Math.abs((-a1-gap)/2));if(!Number.isFinite(r)||r>=1-1e-6||gap<1e-3)return product;
  const h0=b0,h1=b1-a1*h0,h2=b2-a1*h1-a2*h0,h3=-a1*h2-a2*h1;let g:number,tail:number;
  if(d<0){g=2*r/(gap*(1-r));const amplitude=Math.sqrt(h2*h2+((h3+a1*h2/2)/(gap/2))**2);tail=amplitude*r**(k-2)/(1-r);}
  else{const p1=(-a1+gap)/2,p2=(-a1-gap)/2;g=(Math.abs(p1)/(1-Math.abs(p1))+Math.abs(p2)/(1-Math.abs(p2)))/gap;const c1=(h3-p2*h2)/gap,c2=(p1*h2-h3)/gap;tail=Math.abs(c1)*Math.abs(p1)**(k-2)/(1-Math.abs(p1))+Math.abs(c2)*Math.abs(p2)**(k-2)/(1-Math.abs(p2));}
  g*=1+1e-6;const bs=4*(Math.abs(b0)+Math.abs(b1)+Math.abs(b2)+1),aa=4*(Math.abs(a1)+Math.abs(a2)+1);tail=(tail+64*Number.EPSILON*(bs+aa+1)*g)*(1+1e-6);tails+=tail/l;
  const denominator=1-g*gamma*aa;if(denominator<=0)return product;const ln=(l+g*gamma*bs)/denominator,local=g*gamma*(bs+aa*ln)*numericInput+1e-280;numerical=numerical*l+local;numericInput*=ln;
  let x1=0,x2=0,y1=0,y2=0;for(let i=0;i<n;i++){const input=h[i],y=b0*input+b1*x1+b2*x2-a1*y1-a2*y2;x2=x1;x1=input;y2=y1;y1=y;h[i]=y;}
 }
 let prefix=0;for(const x of h)prefix+=Math.abs(x);const candidate=(prefix+product*tails+(n+1)*numerical)*(1+1e-6);return Number.isFinite(candidate)?Math.min(product,Math.max(1,candidate)):product;
}
export function protection(p:EqPreset,rate=48000):import('../types/equalizer').Protection|null {
 if(!validDraft(p)||!Number.isFinite(rate)||rate<=0)return null;let product=1;
 for(const b of p.bands){const l=impulseBound(b,rate);if(l===null)return null;product*=l;}
 const db=20*Math.log10(cascadeBound(p,rate,product)),resampler=20*Math.log10(rate===48000?1:4),effective=Math.min(p.preamp_db,0)-1-db-resampler,attenuation=p.preamp_db-effective;
 if(!Number.isFinite(attenuation)||attenuation>48)return null;
 return{requested_preamp_db:p.preamp_db,effective_preamp_db:effective,attenuation_db:attenuation,filter_bound_db:db,resampler_bound_db:resampler,ceiling_db:-1};
}

export function graphPoint(clientX:number,clientY:number,r:{left:number;top:number;width:number;height:number}){
 const scale=Math.min(r.width/1000,r.height/350);if(!Number.isFinite(scale)||scale<=0)return null;
 return{x:(clientX-r.left-(r.width-1000*scale)/2)/scale,y:(clientY-r.top-(r.height-350*scale)/2)/scale};
}
