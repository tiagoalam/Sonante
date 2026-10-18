import test from 'node:test';import assert from 'node:assert/strict';import {readFileSync,writeFileSync} from 'node:fs';
const css=readFileSync(new URL('../src/index.css',import.meta.url),'utf8');const block=css.split(':root[data-theme="light"] {')[1].split('}')[0];const tokens=Object.fromEntries([...block.matchAll(/(--[\w-]+):\s*(#[\da-fA-F]{6})\s*;/g)].map(m=>[m[1],m[2]]));
const luminance=h=>[1,3,5].map(i=>parseInt(h.slice(i,i+2),16)/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4).reduce((n,v,i)=>n+v*[.2126,.7152,.0722][i],0);
const contrast=(a,b)=>{const[x,y]=[luminance(a),luminance(b)].sort((a,b)=>a-b);return(y+.05)/(x+.05)};
test('light text/state/band pairs retain distinct meanings and sufficient sampled contrast',()=>{
 const rows=[];const check=(fg,bg,min=4.5)=>{const ratio=contrast(tokens['--theme-'+fg],tokens['--theme-'+bg]);assert.ok(ratio>=min,`${fg}/${bg}: ${ratio}`);rows.push({foreground:fg,background:bg,ratio,minimum:min})};
 for(const fg of ['primary','secondary','muted','faint','accent'])for(const bg of ['base','panel','raised','hover'])check('fg-'+fg,'surface-'+bg);
 for(const state of ['error','success','info','purple','pink','warning','accent'])check('fg-'+state,'surface-'+state);
 for(let i=1;i<=10;i++)check('band-'+i,'surface-base');assert.equal(new Set(Array.from({length:10},(_,i)=>tokens['--theme-band-'+(i+1)])).size,10);
 for(const bg of ['base','panel'])check('control-accent','surface-'+bg,3);
 assert.doesNotMatch(css,/filter\s*:\s*(invert|hue-rotate)/);assert.match(css,/data-theme-surface="instrument"/);assert.match(css,/data-theme-surface="contrast"/);
 if(process.env.SONANTE_THEME_CONTRAST_REPORT)writeFileSync(process.env.SONANTE_THEME_CONTRAST_REPORT,JSON.stringify({scope:'specified opaque CSS token pairs; excludes disabled opacity, artwork and technical dark islands; not universal accessibility certification',tokens,pairs:rows},null,2)+'\n');
});
