/** Appearance only. No audio, catalog, navigation or backend configuration. */
export type Appearance = 'dark' | 'light';
export const APPEARANCE_KEY = 'sonante_appearance_v1';
export const APPEARANCE_CHANGED = 'sonante:appearance-changed';
export const APPEARANCE_REQUEST = 'sonante:appearance-request';
export interface AppearanceRecord { version:1; value:Appearance; revision:{counter:number;origin:string} }
interface Environment {
 origin:string;
 storage:()=>Pick<Storage,'getItem'|'setItem'>|null;
 apply:(value:Appearance)=>void;
 onStorage:(receive:(value:string|null)=>void)=>()=>void;
 bridge?:{listen:(event:string,receive:(payload:unknown)=>void)=>Promise<()=>void>;emit:(event:string,payload:unknown)=>Promise<void>};
}
export function appearanceRecord(value:unknown):AppearanceRecord|null {
 if(!value||typeof value!=='object')return null;
 const v=value as Partial<AppearanceRecord>;
 return v.version===1&&(v.value==='dark'||v.value==='light')&&v.revision&&Number.isSafeInteger(v.revision.counter)&&v.revision.counter>=0&&typeof v.revision.origin==='string'&&v.revision.origin.length<=96&&/^[\w-]*$/.test(v.revision.origin)
  ?{version:1,value:v.value,revision:{counter:v.revision.counter,origin:v.revision.origin}}:null;
}
const compare=(a:AppearanceRecord,b:AppearanceRecord)=>a.revision.counter-b.revision.counter||(a.revision.origin<b.revision.origin?-1:a.revision.origin>b.revision.origin?1:0);
export function createAppearance(env:Environment){
 const read=()=>{try{return appearanceRecord(JSON.parse(env.storage()?.getItem(APPEARANCE_KEY)??'null'))}catch{return null}};
 let record:AppearanceRecord=read()??{version:1,value:'dark',revision:{counter:0,origin:''}};
 let generation=0,stop:(()=>void)|null=null;
 const subscribers=new Set<()=>void>();
 const persist=()=>{try{const saved=read();if(!saved||compare(record,saved)>0)env.storage()?.setItem(APPEARANCE_KEY,JSON.stringify(record))}catch{/* Memory preference remains usable. */}};
 const publish=()=>{env.apply(record.value);subscribers.forEach(fn=>fn())};
 const receive=(payload:unknown)=>{const next=appearanceRecord(payload);if(!next)return;if(compare(next,record)>0){record=next;publish()}persist()};
 const emit=(event:string,payload:unknown)=>{try{void env.bridge?.emit(event,payload).catch(()=>{})}catch{/* Appearance is never rolled back by bridge availability. */}};
 return {
  getSnapshot:()=>record.value,
  subscribe:(fn:()=>void)=>{subscribers.add(fn);return()=>{subscribers.delete(fn)}},
  set(value:Appearance){
   if(value!=='dark'&&value!=='light')return;
   const saved=read();if(saved&&compare(saved,record)>0)record=saved;
   record={version:1,value,revision:{counter:Math.max(0,record.revision.counter)+1,origin:env.origin}};
   persist();publish();emit(APPEARANCE_CHANGED,record);
  },
  start(){
   if(stop)return stop;
   const own=++generation,cleanups:(()=>void)[]=[];
   const alive=()=>own===generation;
   const saved=read();if(saved&&compare(saved,record)>0)record=saved;
   publish(); // Synchronous, before React's first render in either window.
   cleanups.push(env.onStorage(value=>{if(!alive())return;try{receive(JSON.parse(value??'null'))}catch{/* Invalid data is ignored during a running session. */}}));
   const register=async()=>{
    if(!env.bridge)return;
    const listeners=[
     env.bridge.listen(APPEARANCE_CHANGED,payload=>{if(alive())receive(payload)}),
     env.bridge.listen(APPEARANCE_REQUEST,()=>{if(alive())emit(APPEARANCE_CHANGED,record)}),
    ];
    const results=await Promise.allSettled(listeners);
    for(const r of results)if(r.status==='fulfilled'){if(alive())cleanups.push(r.value);else r.value()}
    if(alive()&&results.every(r=>r.status==='fulfilled'))emit(APPEARANCE_REQUEST,null);
   };
   void register().catch(()=>{});
   stop=()=>{if(!alive())return;generation++;stop=null;cleanups.splice(0).forEach(fn=>fn())};
   return stop;
  },
 };
}
