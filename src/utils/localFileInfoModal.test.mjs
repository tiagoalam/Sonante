import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
const data = s => `data:text/javascript;base64,${Buffer.from(s).toString("base64")}`;
const read = p => readFileSync(new URL(p, import.meta.url), "utf8");
const hookModule = data(["useEffect","useRef","useState"].map(name=>`export const ${name}=(...args)=>globalThis.__fileInfoHost.${name}(...args);`).join("\n"));
const translated = data('export const useTranslation=()=>({t:(...args)=>globalThis.__fileInfoTranslation.t(...args),i18n:globalThis.__fileInfoTranslation});');
const service = data('export const inspectLocalFile=(...args)=>globalThis.__fileInfoRead(...args);');
const js = ts.transpileModule(read("../components/LocalFileInfoModal.tsx"),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2020}}).outputText
.replace('from "react"',`from ${JSON.stringify(hookModule)}`)
.replace('from "react-i18next"',`from ${JSON.stringify(translated)}`)
.replace('from "../services/localFileInfo"',`from ${JSON.stringify(service)}`)
.replace('from "../utils/localFileInfo"',`from ${JSON.stringify(new URL("./localFileInfo.ts",import.meta.url).href)}`)
.replace(/from "(react\/jsx-runtime|lucide-react)"/g,(_,name)=>`from ${JSON.stringify(import.meta.resolve(name))}`);
const { LocalFileInfoModal } = await import(data(js));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<10;i++)await Promise.resolve();};
function host(props){
 let cursor=0;const slots=[],effects=[];
 const h={slots,
 useState(initial){const n=cursor++;if(!(n in slots))slots[n]=initial;return [slots[n],v=>slots[n]=v];},
 useRef(initial){return slots[cursor++]??={current:initial};},
 useEffect(fn,deps){const n=cursor++,old=slots[n];if(!old||deps.some((v,i)=>v!==old.deps[i]))effects.push(()=>{old?.cleanup?.();slots[n]={deps,cleanup:fn()};});},
 render(next=props){props=next;cursor=0;globalThis.__fileInfoHost=h;return LocalFileInfoModal(props);},
 effects(){effects.splice(0).forEach(fn=>fn());},close(){slots.forEach(s=>s?.cleanup?.());}
 };return h;
}
const info=(format="DSF",rate=2822400,level=64)=>({provenance:"original_file_dsd_header",format,codec:"DSD",compression:"uncompressed",sample_type:"dsd",sample_rate_basis:"encoded_header",original_sample_rate:null,lossy:false,frame_bitrate_kbps:null,sample_rate:rate,bit_depth:null,channels:2,dsd_level:level,duration_seconds:8,revision:"synthetic"});
for(const language of ["pt-BR","en"]){
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read(`../locales/${language}.json`))}},interpolation:{escapeValue:false}});
 test(`actual file modal presents DSD independently of PCM, FLAC and unknowns (${language})`,async()=>{
  globalThis.__fileInfoTranslation=i18n;
  for(const format of ["DSF","DSDIFF"])for(const [rate,level]of [[2822400,64],[5644800,128],[11289600,256],[22579200,null],[2822401,null],[null,null]]){
   const result=info(format,rate,level);result.duration_seconds=null;
   globalThis.__fileInfoRead=async()=>result;
   const h=host({path:"Synthetic/renamed.audio",displayName:"Synthetic",available:true,onClose(){}});h.render();
   // Mount only the request effect: accessibility is exercised separately below.
   h.slots[2].current=null;
   const previous=globalThis.document;globalThis.document={activeElement:null,addEventListener(){},removeEventListener(){}};
   const previousElement=globalThis.HTMLElement;globalThis.HTMLElement=class{};
   try{h.effects();await flush();const text=renderToStaticMarkup(h.render());
    assert.ok(text.includes(format));assert.ok(text.includes(i18n.t("fileInfo.dsdProvenance")));
    if(level!==null)assert.ok(text.includes(`DSD${level}`));else assert.ok(text.includes(i18n.t("fileInfo.unknownLevel")));
    if(rate!==null){assert.ok(text.includes(`${rate} Hz`));assert.ok(text.includes(`${(rate/1e6).toLocaleString(language,{maximumFractionDigits:6})} MHz`));}
    assert.ok(!text.includes(i18n.t("fileInfo.bitDepth")));assert.ok(!text.includes(i18n.t("fileInfo.bitDepthValue",{bits:8})));
    assert.ok(!text.includes("NaN"));assert.ok(text.includes(i18n.t("fileInfo.unknown")));
   }finally{h.close();globalThis.document=previous;globalThis.HTMLElement=previousElement;}
  }
  const flac={...info(),provenance:"original_file_streaminfo",format:"FLAC",codec:"FLAC",compression:"lossless",sample_type:"integer",sample_rate:96000,bit_depth:24,dsd_level:null};
  globalThis.__fileInfoRead=async()=>flac;const h=host({path:"Synthetic/a.flac",displayName:"FLAC",available:true,onClose(){}});
  const prev={document:globalThis.document,HTMLElement:globalThis.HTMLElement};globalThis.document={activeElement:null,addEventListener(){},removeEventListener(){}};globalThis.HTMLElement=class{};
  try{h.render();h.effects();await flush();const text=renderToStaticMarkup(h.render());assert.ok(text.includes("96000 Hz"));assert.ok(text.includes(i18n.t("fileInfo.bitDepthValue",{bits:24})));assert.ok(text.includes(i18n.t("fileInfo.provenance")));assert.ok(!text.includes(i18n.t("fileInfo.dsdLevel")));}finally{h.close();Object.assign(globalThis,prev);}
 });
 test(`actual modal rejects late replies on path/invalidation/unmount and restores keyboard focus (${language})`,async()=>{
  globalThis.__fileInfoTranslation=i18n;const requests=[],calls=[];let closes=0,focused=0,closeFocus=0;
  globalThis.__fileInfoRead=(path,context)=>{const d=deferred();requests.push(d);calls.push([path,context]);return d.promise;};
  class Element{focus(){focused++;}querySelector(){return {focus(){closeFocus++;}};}}
  const previous=new Element(),listeners=new Map(),prev={document:globalThis.document,HTMLElement:globalThis.HTMLElement};
  globalThis.HTMLElement=Element;globalThis.document={activeElement:previous,addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:(name,fn)=>{assert.equal(listeners.get(name),fn);listeners.delete(name);}};
  const base={path:"Synthetic/a.dsf",displayName:"A",available:true,onClose(){closes++;}},h=host(base);
  try{
   h.render();h.slots[2].current=new Element();h.effects();assert.equal(focused,1);assert.equal(calls.length,1);
   let prevented=0,stopped=0;listeners.get("keydown")({key:"Tab",preventDefault(){prevented++;}});assert.equal(prevented,1);assert.equal(closeFocus,1);
   listeners.get("keydown")({key:"Escape",stopPropagation(){stopped++;}});assert.equal(closes,1);assert.equal(stopped,1);
   h.render({...base,path:"Synthetic/b.dff",displayName:"B"});h.effects();requests[0].resolve(info("DSF"));await flush();assert.ok(!renderToStaticMarkup(h.render()).includes("DSD64"));
   requests[1].resolve(info("DSDIFF",11289600,256));await flush();assert.ok(renderToStaticMarkup(h.render()).includes("DSD256"));
   h.render({...base,path:"Synthetic/b.dff",available:false});h.effects();assert.ok(renderToStaticMarkup(h.render()).includes(i18n.t("fileInfo.errors.stale")));assert.equal(calls.length,2);
   h.render({...base,path:"Synthetic/c.dsf"});h.effects();const before=h.slots[0];h.close();requests[2].resolve(info());await flush();assert.equal(h.slots[0],before);assert.equal(focused,2);assert.equal(listeners.size,0);
  }finally{Object.assign(globalThis,prev);}
 });
}

for(const language of ["pt-BR","en"]){
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read(`../locales/${language}.json`))}},interpolation:{escapeValue:false}});
 test(`actual MP3 modal labels lossy, non-PCM depth, frame bitrate and unmeasured duration (${language})`,async()=>{
  globalThis.__fileInfoTranslation=i18n;
  const prev={document:globalThis.document,HTMLElement:globalThis.HTMLElement};globalThis.document={activeElement:null,addEventListener(){},removeEventListener(){}};globalThis.HTMLElement=class{};
  try{for(const rate of [44100,22050,11025])for(const kbps of [320,128,null]){
   const mp3={...info(),provenance:"original_file_mpeg_headers",format:"MPEG Audio",codec:"MP3",compression:"lossy",sample_type:"not_applicable",lossy:true,frame_bitrate_kbps:kbps,bit_depth:null,dsd_level:null,sample_rate:rate,duration_seconds:null};
   globalThis.__fileInfoRead=async()=>mp3;const h=host({path:"Synthetic/not-extension.flac",displayName:"MP3",available:true,onClose(){}});
   h.render();h.effects();await flush();const text=renderToStaticMarkup(h.render());
   for(const key of ["mp3Provenance","compression","lossy","notApplicable","frameBitrate","frameBitrateHint","mp3DurationUnknown"])assert.ok(text.includes(i18n.t(`fileInfo.${key}`)),key);
   if(kbps!==null)assert.ok(text.includes(`${kbps} kbps`));else assert.ok(text.includes(i18n.t("fileInfo.unknown")));
   assert.ok(text.includes(`${rate} Hz`));
   for(const bits of [8,16,24,32])assert.ok(!text.includes(i18n.t("fileInfo.bitDepthValue",{bits})));
   assert.ok(!text.includes("DSD64"));assert.ok(!text.includes("Hi-Res"));h.close();
  }}finally{Object.assign(globalThis,prev);}
 });
 test(`late MP3 replies never replace a new file or closed dialog (${language})`,async()=>{
  globalThis.__fileInfoTranslation=i18n;const prev={document:globalThis.document,HTMLElement:globalThis.HTMLElement};globalThis.document={activeElement:null,addEventListener(){},removeEventListener(){}};globalThis.HTMLElement=class{};
  const requests=[];globalThis.__fileInfoRead=()=>{const d=deferred();requests.push(d);return d.promise;};
  const base={path:"Synthetic/a.mp3",displayName:"A",available:true,onClose(){}},h=host(base),mp3={...info(),provenance:"original_file_mpeg_headers",format:"MPEG Audio",codec:"MP3",compression:"lossy",sample_type:"not_applicable",lossy:true,frame_bitrate_kbps:320,bit_depth:null,dsd_level:null,duration_seconds:null};
  try{h.render();h.effects();h.render({...base,path:"Synthetic/b.flac"});h.effects();requests[0].resolve(mp3);await flush();assert.ok(!renderToStaticMarkup(h.render()).includes("320 kbps"));
   h.close();const before=h.slots[0];requests[1].resolve({...mp3,frame_bitrate_kbps:128});await flush();assert.equal(h.slots[0],before);
  }finally{Object.assign(globalThis,prev);}
 });
}

for (const language of ["pt-BR", "en"]) {
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read(`../locales/${language}.json`))}},interpolation:{escapeValue:false}});
 test(`actual format matrix preserves compression, representation, provenance and distinct clocks (${language})`,async()=>{
  globalThis.__fileInfoTranslation=i18n;
  const prev={document:globalThis.document,HTMLElement:globalThis.HTMLElement};globalThis.document={activeElement:null,addEventListener(){},removeEventListener(){}};globalThis.HTMLElement=class{};
  const cases=[
   {format:"WAV",codec:"PCM",compression:"uncompressed",sample_type:"integer",sample_rate:96000,bit_depth:16},
   {format:"WAV",codec:"PCM float",compression:"uncompressed",sample_type:"float",sample_rate:48000,bit_depth:32},
   {format:"AIFF-C",codec:"PCM",compression:"uncompressed",sample_type:"integer",sample_rate:44100,bit_depth:24},
   {format:"ADTS",codec:"AAC",compression:"lossy",sample_type:"not_applicable",sample_rate:22050,bit_depth:null,sample_rate_basis:"aac_core_header"},
   {format:"MP4/M4A",codec:"AAC",compression:"lossy",sample_type:"not_applicable",sample_rate:44100,bit_depth:null},
   {format:"MP4/M4A",codec:"ALAC",compression:"lossless",sample_type:"integer",sample_rate:96000,bit_depth:24},
   {format:"Ogg",codec:"Vorbis",compression:"lossy",sample_type:"not_applicable",sample_rate:44100,bit_depth:null},
   {format:"Ogg",codec:"FLAC",compression:"lossless",sample_type:"integer",sample_rate:96000,bit_depth:24},
   {format:"Ogg",codec:"Opus",compression:"lossy",sample_type:"not_applicable",sample_rate:48000,bit_depth:null,sample_rate_basis:"opus_decode_clock",original_sample_rate:12345},
   {format:"WavPack",codec:"WavPack",compression:"hybrid",sample_type:"integer",sample_rate:96000,bit_depth:null},
   {format:"Musepack SV8",codec:"Musepack",compression:"lossy",sample_type:"not_applicable",sample_rate:44100,bit_depth:null},
   {format:"WavPack",codec:"WavPack",compression:"unknown",sample_type:"integer",sample_rate:null,bit_depth:null},
  ];
  try{for(const entry of cases){
   const result={...info(),dsd_level:null,provenance:"original_file_container_headers",duration_seconds:null,...entry};
   globalThis.__fileInfoRead=async()=>result;const h=host({path:"Synthetic/renamed.bin",displayName:"Synthetic",available:true,onClose(){}});
   try{h.render();h.effects();await flush();const text=renderToStaticMarkup(h.render());
    assert.ok(text.includes(entry.codec));assert.ok(text.includes(entry.format));assert.ok(text.includes(i18n.t(`fileInfo.compressionValues.${entry.compression}`)));assert.ok(text.includes(i18n.t("fileInfo.containerProvenance")));
    if(entry.sample_type==="not_applicable")assert.ok(text.includes(i18n.t("fileInfo.notApplicable")));
    if(entry.bit_depth!==null)assert.ok(text.includes(i18n.t("fileInfo.bitDepthValue",{bits:entry.bit_depth})));
    if(entry.sample_type==="float")for(const key of ["floatDepth","floatHint"])assert.ok(text.includes(i18n.t(`fileInfo.${key}`)));
    if(entry.codec==="Opus"){for(const key of["opusClock","originalRate","opusHint"])assert.ok(text.includes(i18n.t(`fileInfo.${key}`)));assert.ok(text.includes("48000 Hz"));assert.ok(text.includes("12345 Hz"));}
    if(entry.sample_rate_basis==="aac_core_header")for(const key of["aacCoreRate","aacCoreHint"])assert.ok(text.includes(i18n.t(`fileInfo.${key}`)));
    if(entry.compression==="hybrid")assert.ok(text.includes(i18n.t("fileInfo.hybridHint")));
    assert.ok(!text.includes("Hi-Res"));assert.ok(!text.includes("DSD64"));assert.ok(!text.includes("NaN"));
   }finally{h.close();}
  }}finally{Object.assign(globalThis,prev);}
 });
}
