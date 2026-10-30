import test from 'node:test';import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';import {createAppearance,APPEARANCE_KEY,APPEARANCE_CHANGED,APPEARANCE_REQUEST} from './appearance.ts';
const flush=async()=>{for(let i=0;i<30;i++)await Promise.resolve()};
const record=(value,counter=1,origin='fixture')=>({version:1,value,revision:{counter,origin}});
function fixture(raw=null,extra={}){let saved=raw;const applied=[],handlers=new Set();const storage={getItem:()=>saved,setItem:(k,v)=>{assert.equal(k,APPEARANCE_KEY);saved=v}};const env={origin:'fixture',storage:()=>storage,apply:v=>applied.push(v),onStorage:fn=>{handlers.add(fn);return()=>handlers.delete(fn)},...extra};return {env,applied,handlers,store:createAppearance(env),raw:()=>saved};}
test('missing, invalid and inaccessible storage default to Dark; valid choice applies synchronously before render',()=>{
 for(const raw of [null,'bad','"light"',JSON.stringify(record('system')),JSON.stringify(record('light',NaN)),JSON.stringify({...record('light'),version:2})]){const f=fixture(raw);const stop=f.store.start();assert.equal(f.store.getSnapshot(),'dark');assert.deepEqual(f.applied,['dark']);stop()}
 const f=fixture(JSON.stringify(record('light')));f.store.start();assert.deepEqual(f.applied,['light']);f.store.set('dark');assert.equal(JSON.parse(f.raw()).value,'dark');assert.equal(f.store.getSnapshot(),'dark');
 const blocked=fixture(null,{storage(){throw Error('blocked')}});const stop=blocked.store.start();blocked.store.set('light');assert.equal(blocked.store.getSnapshot(),'light');stop();
});
test('late older bridge replies and storage events never undo rapid local choices',async()=>{
 const listeners=new Map(),sent=[];const f=fixture(null,{bridge:{listen:async(name,fn)=>{listeners.set(name,fn);return()=>listeners.delete(name)},emit:async(...v)=>sent.push(v)}});const stop=f.store.start();await flush();assert.equal(sent[0][0],APPEARANCE_REQUEST);f.store.set('light');f.store.set('dark');f.store.set('light');const current=JSON.parse(f.raw());
 listeners.get(APPEARANCE_CHANGED)(record('dark',1));assert.equal(f.store.getSnapshot(),'light');for(const fn of f.handlers)fn(JSON.stringify(record('dark',2)));assert.equal(f.store.getSnapshot(),'light');listeners.get(APPEARANCE_CHANGED)(record('dark',current.revision.counter+1));assert.equal(f.store.getSnapshot(),'dark');stop();assert.equal(listeners.size,0);
});
test('new and already-open windows converge through handshake, shared storage and reordered messages',async()=>{
 let saved=null;const listeners=[],queue=[];const storage={getItem:()=>saved,setItem:(_k,v)=>saved=v};
 const window=(origin)=>fixture(null,{origin,storage:()=>storage,bridge:{listen:async(name,fn)=>{const entry={name,fn};listeners.push(entry);return()=>listeners.splice(listeners.indexOf(entry),1)},emit:async(name,payload)=>queue.push({name,payload:structuredClone(payload)})}});
 const deliver=()=>{let count=0;while(queue.length){assert.ok(count++<100);const e=queue.shift();for(const l of [...listeners])if(e.name===l.name)l.fn(e.payload)}};
 const a=window('a'),stopA=a.store.start();await flush();deliver();a.store.set('light');deliver();const b=window('b');assert.equal(b.store.getSnapshot(),'light');const stopB=b.store.start();await flush();deliver();assert.equal(b.store.getSnapshot(),'light');a.store.set('dark');a.store.set('light');b.store.set('dark');queue.reverse();deliver();assert.equal(a.store.getSnapshot(),'dark');assert.equal(b.store.getSnapshot(),'dark');assert.equal(JSON.parse(saved).value,'dark');stopA();stopB();assert.equal(listeners.length,0);
});
test('storage-free windows still synchronize; old async listeners clean up after stop/restart',async()=>{
 const pending=[],listeners=new Map();const f=fixture(null,{storage:()=>null,bridge:{listen:(name,fn)=>new Promise(resolve=>pending.push(()=>{listeners.set(name,fn);resolve(()=>listeners.delete(name))})),emit:async()=>{}}});const oldStop=f.store.start();assert.equal(f.store.start(),oldStop);oldStop();for(const fn of pending.splice(0))fn();await flush();assert.equal(listeners.size,0);const stop=f.store.start();oldStop();assert.equal(f.handlers.size,1);for(const fn of pending.splice(0))fn();await flush();listeners.get(APPEARANCE_CHANGED)(record('light',4));assert.equal(f.store.getSnapshot(),'light');stop();assert.equal(listeners.size,0);assert.equal(f.handlers.size,0);
});
test('deterministic concurrent revision tie converges and repairs a losing persisted record',async()=>{
 let saved=JSON.stringify(record('dark',3,'a'));const listeners={};const f=fixture(saved,{origin:'b',storage:()=>({getItem:()=>saved,setItem:(_k,v)=>saved=v}),bridge:{listen:async(n,fn)=>{listeners[n]=fn;return()=>{}},emit:async()=>{}}});f.store.start();await flush();listeners[APPEARANCE_CHANGED](record('light',3,'z'));assert.equal(f.store.getSnapshot(),'light');saved=JSON.stringify(record('dark',3,'a'));listeners[APPEARANCE_CHANGED](record('dark',3,'a'));assert.equal(JSON.parse(saved).value,'light');assert.equal(f.store.getSnapshot(),'light');
});
test('bootstrap and appearance service have no audio/catalog/config commands or remount keys',()=>{
 const read=p=>readFileSync(new URL(p,import.meta.url),'utf8');const main=read('../main.tsx'),bridge=read('../services/appearance.ts'),control=read('../components/AppearanceSetting.tsx'),model=read('./appearance.ts');assert.ok(main.indexOf('appearance.start()')<main.indexOf('ReactDOM.createRoot'));assert.match(main,/isNowPlayingWindow/);for(const text of [bridge,control,model])assert.doesNotMatch(text,/save_config|audioService|configService|plexService|localAlbumCatalog|invoke\(|\.play\(|\.seek\(|setVolume/);assert.match(control,/useSyncExternalStore/);assert.doesNotMatch(main,/key=.*appearance/);
});
test('pre-paint HTML uses the same validation as the model, including storage failures',()=>{
 const html=readFileSync(new URL('../../index.html',import.meta.url),'utf8');const boot=html.match(/<script>([\s\S]*?)<\/script>/)[1];assert.ok(html.indexOf('<script>')<html.indexOf('type="module"'));
 for(const raw of [null,'bad','"light"',JSON.stringify(record('light')),JSON.stringify(record('dark')),JSON.stringify(record('system')),JSON.stringify(record('light',-1)),JSON.stringify(record('light',1,'bad origin')),JSON.stringify({...record('light'),version:2})]){
  const f=fixture(raw),doc={documentElement:{dataset:{},style:{}}};new Function('localStorage','document',boot)({getItem:()=>raw},doc);assert.equal(doc.documentElement.dataset.theme,f.store.getSnapshot());assert.equal(doc.documentElement.style.colorScheme,f.store.getSnapshot());
 }const doc={documentElement:{dataset:{},style:{}}};new Function('localStorage','document',boot)({getItem(){throw Error('blocked')}},doc);assert.equal(doc.documentElement.dataset.theme,'dark');
});
