/** Presentation only. Formatting never becomes a preset/DSP value. */
export function formatEqNumber(value:number,digits:number,locale='en') {
 return Number.isFinite(value)?new Intl.NumberFormat(locale,{useGrouping:false,maximumFractionDigits:digits}).format(Object.is(value,-0)?0:value):'';
}
export function parseEqNumber(text:string) {
 const value=text.trim();
 return /^[+-]?(?:\d+(?:[.,]\d*)?|[.,]\d+)(?:[eE][+-]?\d+)?$/.test(value)?Number(value.replace(',','.')):NaN;
}
