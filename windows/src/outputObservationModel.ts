export type EndpointVolume = Readonly<{value:number;muted:boolean}>;
export type OutputObservation = Readonly<{deviceName:string;observedAt:number;endpointVolume?:EndpointVolume}>;
export type OutputResult = {kind:"observed";value:OutputObservation}|{kind:"unavailable";reason:string};
const reasons=new Set(["busy","changed","timeout","ambiguous","not_observed","unavailable","unreadable","limit"]);
export function decodeObservation(value:unknown):OutputObservation {
 if(!value||typeof value!=="object")throw "unreadable";
 const item=value as Partial<OutputObservation>;
 if(typeof item.deviceName!=="string"||!item.deviceName.trim()||item.deviceName.length>1024||/[\u0000-\u001f\u007f-\u009f]/.test(item.deviceName)||typeof item.observedAt!=="number"||!Number.isSafeInteger(item.observedAt)||item.observedAt<0||item.observedAt>253402300799999)throw "unreadable";
 let endpointVolume:EndpointVolume|undefined;
 if(item.endpointVolume!=null){const level=item.endpointVolume;if(typeof level!=="object"||typeof level.value!=="number"||!Number.isInteger(level.value)||level.value<0||level.value>100||typeof level.muted!=="boolean")throw "unreadable";endpointVolume={value:level.value,muted:level.muted};}
 return {deviceName:item.deviceName,observedAt:item.observedAt,...(endpointVolume?{endpointVolume}:{})};
}
// The deadline releases only presentation, never the native permit. Closing,
// changing session/cursor or timing out must discard a late COM response.
export function requestObservation(read:()=>Promise<unknown>,publish:(result:OutputResult)=>void,timeoutMs=5000):()=>void {
 let active=true;
 const finish=(result:OutputResult)=>{if(!active)return;active=false;clearTimeout(timer);publish(result)};
 const timer=setTimeout(()=>finish({kind:"unavailable",reason:"timeout"}),timeoutMs);
 void Promise.resolve().then(()=>active?read():undefined).then(value=>finish({kind:"observed",value:decodeObservation(value)})).catch(error=>finish({kind:"unavailable",reason:typeof error==="string"&&reasons.has(error)?error:"unavailable"}));
 return ()=>{active=false;clearTimeout(timer)};
}
