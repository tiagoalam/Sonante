export interface IndexCoverageCounts {source:string;audio_files:number;cue_groups:number;cataloged_audio:number;virtual_tracks:number;omitted:number;policy_excluded:number;audio_unconfirmed?:number;cue_groups_unobserved?:number;unaddressable?:number}
export interface IndexCoverageReport {outcome:'confirmed'|'partial';generation:string;scope:'full'|'paths';sources:IndexCoverageCounts[]}
export interface IndexCoverageSnapshot {revision:number;report:IndexCoverageReport|null}
export function coverageSnapshot(value:unknown):IndexCoverageSnapshot|null {
 if(!value||typeof value!=='object')return null;const s=value as IndexCoverageSnapshot;
 if(!Number.isSafeInteger(s.revision)||s.revision<0)return null;
 if(s.report===null)return{revision:s.revision,report:null};const r=s.report;
 if(!r||!['confirmed','partial'].includes(r.outcome)||!['full','paths'].includes(r.scope)||typeof r.generation!=='string'||!Array.isArray(r.sources))return null;
 if(!r.sources.every(c=>c&&typeof c.source==='string'&&c.source.length>0&&!/[\r\n\0/:]/.test(c.source)&&['audio_files','cue_groups','cataloged_audio','virtual_tracks','omitted','policy_excluded'].every(k=>Number.isSafeInteger(c[k as keyof IndexCoverageCounts])&&(c[k as keyof IndexCoverageCounts] as number)>=0)))return null;
 if(!r.sources.every(c=>{
  const values=[c.audio_unconfirmed,c.cue_groups_unobserved,c.unaddressable];
  // Older reports retain their aggregate. Never invent a category for them.
  if(values.every(v=>v===undefined))return true;
  if(!values.every(v=>Number.isSafeInteger(v)&&(v as number)>=0))return false;
  const [audio,cues,paths]=values as number[];
  return audio===c.audio_files-c.cataloged_audio && cues<=c.cue_groups
   && audio+cues+paths===c.omitted;
 }))return null;
 return{revision:s.revision,report:r};
}
/** One event subscription and initial query for all mounted consumers. No polling. */
export function createIndexCoverageStore(read:()=>Promise<unknown>,listen:(receive:(snapshot:unknown)=>void)=>Promise<()=>void>){
 let snapshot:IndexCoverageSnapshot={revision:0,report:null},generation=0;const subscribers=new Set<()=>void>();let cleanup:(()=>void)|undefined;
 const receive=(value:unknown)=>{const next=coverageSnapshot(value);if(!next)return;if(next.revision<snapshot.revision||next.revision===snapshot.revision&&(snapshot.report!==null||next.report===null))return;snapshot=next;for(const fn of subscribers)fn()};
 const start=()=>{const own=++generation;void(async()=>{const stop=await listen(value=>{if(own===generation)receive(value)});if(own!==generation){stop();return}cleanup=stop;const sampled=own;const value=await read();if(sampled===generation)receive(value)})().catch(()=>{/* Unknown remains unknown; never infer absence of omissions. */})};
 return{getSnapshot:()=>snapshot,subscribe(fn:()=>void){subscribers.add(fn);if(subscribers.size===1)start();return()=>{subscribers.delete(fn);if(!subscribers.size){generation++;cleanup?.();cleanup=undefined}}}};
}
