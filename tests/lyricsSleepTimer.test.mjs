import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import * as lyrics from '../src/utils/trackLyrics.ts';
import { SleepTimerStore, decodeSleepTimer, remainingSleepMinutes } from '../src/utils/sleepTimer.ts';
const flush = async () => { for(let i=0;i<50;i++) await Promise.resolve(); };
const deferred = () => { let resolve,reject; const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}; };
const query = (title='Synthetic Song') => ({title,artist:'Synthetic Artist',album:'Synthetic Album',duration:120});
const record = (q=query(),extra={}) => ({trackName:q.title,artistName:q.artist,albumName:q.album,duration:120,plainLyrics:'Synthetic first line\nSynthetic second line',...extra});
const response = (value,status=200,headers={}) => new Response(JSON.stringify(value),{status,headers:{'Content-Type':'application/json',...headers}});
const storage = () => {const data=new Map();return{data,getItem:key=>data.get(key)??null,setItem:(key,value)=>data.set(key,value)}};
function compile(file,deps) { const result={};new Function('require','exports',ts.transpileModule(readFileSync(new URL('../src/'+file,import.meta.url),'utf8'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},result);return result; }
function hooks() { let cursor=0;const slots=[],effects=[];return {react:{...React,default:React,useState(initial){const n=cursor++;if(!(n in slots))slots[n]=typeof initial==='function'?initial():initial;return [slots[n],value=>{slots[n]=typeof value==='function'?value(slots[n]):value}]},useRef(initial){return slots[cursor++]??={current:initial}},useEffect(fn,deps){const n=cursor++,previous=slots[n];if(!previous||deps.some((value,i)=>!Object.is(value,previous.deps[i])))effects.push(()=>{previous?.cleanup?.();slots[n]={deps,cleanup:fn()}})}},render(fn){cursor=0;const result=fn();effects.splice(0).forEach(fn=>fn());return result},dispose(){slots.forEach(slot=>slot?.cleanup?.())}}; }
function nodes(tree,predicate,result=[]) {if(!tree||typeof tree!=='object')return result;if(Array.isArray(tree)){tree.forEach(child=>nodes(child,predicate,result));return result}if(predicate(tree))result.push(tree);nodes(tree.props?.children,predicate,result);return result;}
function text(tree) { if(tree===null||tree===undefined||typeof tree==='boolean')return '';if(typeof tree==='string'||typeof tree==='number')return String(tree);if(Array.isArray(tree))return tree.map(text).join(' ');return text(tree.props?.children); }

test('lyrics query transmits whitelisted metadata only, excludes radio/paths, and checks version identity',()=>{
 assert.equal(lyrics.lyricsQuery({title:'Live',artist:'Station',current_media:{kind:'radio'}}),null);
 for(const title of ['/personal/file.flac','https://private.invalid/?token=SECRET','file:///path','bad\nname'])assert.equal(lyrics.lyricsQuery({...query(),title}),null);
 const value=lyrics.lyricsQuery({...query(),uri:'/personal/path',token:'SECRET',elapsed:50,current_media:{kind:'plex',part_key:'/private'}});
 assert.deepEqual(value,query());assert.deepEqual(Object.keys(value),['title','artist','album','duration']);
 assert.deepEqual(lyrics.parseLyrics(record(),query()),{kind:'found',text:'Synthetic first line\nSynthetic second line'});
 for(const changes of [{artistName:'Other'},{trackName:'Other'},{duration:123},{duration:NaN}])assert.deepEqual(lyrics.parseLyrics(record(query(),changes),query()),{kind:'not_found'});
 assert.deepEqual(lyrics.parseLyrics(record(query(),{instrumental:true}),query()),{kind:'instrumental'});
 assert.deepEqual(lyrics.parseLyrics(record(query(),{plainLyrics:null,syncedLyrics:'[ar:Synthetic]\n[00:01.20]First\n[00:02.00]Second'}),query()),{kind:'found',text:'First\nSecond'});
 assert.throws(()=>lyrics.parseLyrics(record(query(),{plainLyrics:'x'.repeat(32001)}),query()));
 assert.equal(lyrics.lyricsKey({...query(),title:' SYNTHETIC   Song '}),lyrics.lyricsKey(query()));
});

test('lyrics request is independent, bounded and cached across sessions without refetching or leaking locators',async()=>{
 let now=1_000_000,calls=0;const saved=storage();const client=new lyrics.LyricsClient(async(url,options)=>{
  calls++;const parsed=new URL(url);assert.equal(parsed.origin,'https://lrclib.net');assert.equal(parsed.pathname,'/api/get');assert.deepEqual([...parsed.searchParams.keys()],['track_name','artist_name','album_name','duration']);
  assert.equal(options.credentials,'omit');assert.equal(options.redirect,'error');assert.match(options.headers['X-User-Agent'],/Sonante/);return response(record());
 },()=>saved,()=>now);
 const result=await client.lookup(query(),new AbortController().signal);assert.equal(result.kind,'found');
 assert.deepEqual(await client.lookup(query(),new AbortController().signal),result);assert.equal(calls,1);
 const reopened=new lyrics.LyricsClient(()=>assert.fail('cache must survive a new reader'),()=>saved,()=>now);
 assert.deepEqual(await reopened.lookup(query(),new AbortController().signal),result);
 now+=31*86400_000;const expired=new lyrics.LyricsClient(async()=>{calls++;return response(record())},()=>saved,()=>now);
 await expired.lookup(query(),new AbortController().signal);assert.equal(calls,2);
 assert.doesNotMatch(saved.data.get(lyrics.LYRICS_CACHE_KEY),/private|token|uri/);
});

test('lyrics deduplicates live consumers, cancels abandoned/queued work and never lets aborted requests populate cache',async()=>{
 let now=1000;const jobs=[];const saved=storage();const client=new lyrics.LyricsClient((url,{signal})=>{
  const job={...deferred(),url,signal};jobs.push(job);signal.addEventListener('abort',()=>job.reject(new DOMException('Canceled','AbortError')),{once:true});return job.promise;
 },()=>saved,()=>now);
 const first=new AbortController(),second=new AbortController(),queued=new AbortController();
 const p1=client.lookup(query(),first.signal),p2=client.lookup(query(),second.signal);await flush();assert.equal(jobs.length,1);
 first.abort();await assert.rejects(p1,{name:'AbortError'});assert.equal(jobs[0].signal.aborted,false,'another consumer retains the in-flight request');
 const abandoned=client.lookup(query('Abandoned'),queued.signal);queued.abort();await assert.rejects(abandoned,{name:'AbortError'});
 jobs[0].resolve(response(record()));assert.equal((await p2).kind,'found');now+=500;await flush();assert.equal(jobs.length,1,'aborted queued metadata never goes online');
 const last=new AbortController(),p3=client.lookup(query('New Song'),last.signal);await flush();assert.equal(jobs.length,2);last.abort();await assert.rejects(p3,{name:'AbortError'});assert.ok(jobs[1].signal.aborted);await flush();
 assert.doesNotMatch(saved.data.get(lyrics.LYRICS_CACHE_KEY),/New Song|Abandoned/);
});

test('provider failures are not cached as missing; miss TTL, body bounds and Retry-After are respected',async()=>{
 let now=1_000_000,calls=0;const saved=storage();let mode='missing';
 const client=new lyrics.LyricsClient(async()=>{calls++;if(mode==='missing')return response({},404);if(mode==='error')return response({},500);if(mode==='limited')return response({},429,{'Retry-After':'180'});if(mode==='large')return response(record(),200,{'Content-Length':String(300*1024)});return response(record())},()=>saved,()=>now);
 assert.deepEqual(await client.lookup(query(),new AbortController().signal),{kind:'not_found'});await client.lookup(query(),new AbortController().signal);assert.equal(calls,1);
 now+=3600_001;mode='error';await assert.rejects(client.lookup(query(),new AbortController().signal));now+=61001;mode='large';await assert.rejects(client.lookup(query(),new AbortController().signal));
 now+=500;mode='limited';await assert.rejects(client.lookup(query(),new AbortController().signal));const before=calls;
 now+=120000;mode='ok';await assert.rejects(client.lookup(query(),new AbortController().signal));assert.equal(calls,before,'no request while Retry-After remains active');now+=62000;
 assert.equal((await client.lookup(query(),new AbortController().signal)).kind,'found');
 const hostile=new lyrics.LyricsClient(async()=>new Response(new Uint8Array(300*1024),{headers:{'Content-Type':'application/json'}}),()=>null,()=>now);
 await assert.rejects(hostile.lookup(query(),new AbortController().signal),'stream length bounded even without Content-Length');
});

test('lyrics cache stays bounded and quota failures retain a usable session cache',async()=>{
 const saved=storage();let now=100000,calls=0;
 const client=new lyrics.LyricsClient(async url=>{calls++;const q=query(new URL(url).searchParams.get('track_name'));return response(record(q,{plainLyrics:'s'.repeat(30000)}))},()=>saved,()=>now);
 for(let i=0;i<25;i++){now+=500;await client.lookup(query(`Song ${i}`),new AbortController().signal)}
 const raw=saved.data.get(lyrics.LYRICS_CACHE_KEY);assert.ok(raw.length<=280000);assert.ok(JSON.parse(raw).length<=20);assert.equal(calls,25);
 const blocked=new lyrics.LyricsClient(async()=>{calls++;return response(record())},()=>({getItem(){throw Error('blocked')},setItem(){throw Error('quota')}}),()=>now);
 await blocked.lookup(query(),new AbortController().signal);await blocked.lookup(query(),new AbortController().signal);assert.equal(calls,26);
});

test('actual Sleeve lyrics stay opt-in, ignore late old tracks and do not refetch on elapsed/analyzer updates',async()=>{
 const oldWindow=globalThis.window;const stored=storage(),timers=new Map(),listeners=new Map(),jobs=[];let id=0;
 try {
  globalThis.window={localStorage:stored,setTimeout:fn=>{timers.set(++id,fn);return id},clearTimeout:id=>timers.delete(id),addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:(name,fn)=>{if(listeners.get(name)===fn)listeners.delete(name)}};
  const h=hooks();const {TrackLyrics}=compile('components/TrackLyrics.tsx',{
   react:h.react,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:key=>key})},'../utils/trackLyrics':{...lyrics,trackLyrics:{lookup(q,signal){const job={...deferred(),q,signal};jobs.push(job);return job.promise}}}
  });
  let track={...query(),current_media:{kind:'local'}};const render=()=>h.render(()=>TrackLyrics({track}));let tree=render();assert.equal(timers.size,0);assert.equal(jobs.length,0);
  nodes(tree,n=>n.type==='input')[0].props.onChange({target:{checked:true}});tree=render();assert.equal(stored.getItem(lyrics.LYRICS_ENABLED_KEY),'true');
  for(const [id,fn]of [...timers]){timers.delete(id);fn()}await flush();assert.equal(jobs.length,1);
  track={...track,title:'Next Song'};tree=render();assert.ok(jobs[0].signal.aborted);jobs[0].resolve({kind:'found',text:'OLD LYRICS'});await flush();tree=render();assert.doesNotMatch(text(tree),/OLD LYRICS/);
  for(const[id,fn]of [...timers]){timers.delete(id);fn()}jobs[1].resolve({kind:'found',text:'CURRENT LYRICS'});await flush();tree=render();assert.match(text(tree),/CURRENT LYRICS/);
  for(let i=0;i<30;i++){track={...track,elapsed:i,processing:{sample_rate:48000}};tree=render()}assert.equal(jobs.length,2);assert.equal(timers.size,0);
  nodes(tree,n=>n.type==='input')[0].props.onChange({target:{checked:false}});tree=render();assert.doesNotMatch(text(tree),/CURRENT LYRICS/);
  h.dispose();assert.equal(timers.size,0);assert.equal(listeners.size,0);
 } finally {globalThis.window=oldWindow}
});

test('native timer store shares one subscription, fences old replies and only confirmed setters change the deadline',async()=>{
 const firstRead=deferred(),writes=[],registration=deferred();let event,reads=0,unlisten=0,now=100;
 const store=new SleepTimerStore({read:()=>{reads++;return firstRead.promise},set:minutes=>{const job={...deferred(),minutes};writes.push(job);return job.promise},listen:fn=>{event=fn;return registration.promise}},()=>now);
 const off1=store.subscribe(()=>{}),off2=store.subscribe(()=>{});registration.resolve(()=>unlisten++);await flush();assert.equal(reads,1);
 event({revision:2,phase:'armed',remaining_ms:900000});firstRead.resolve({revision:0,phase:'idle',remaining_ms:0});await flush();assert.equal(store.getSnapshot().timer.revision,2);
 now+=60000;assert.equal(remainingSleepMinutes(store.getSnapshot(),now),14);
 const replacing=store.change(30);assert.equal(store.getSnapshot().changing,true);assert.equal(store.getSnapshot().timer.revision,2,'no optimistic timer');writes[0].resolve({revision:3,phase:'armed',remaining_ms:1800000});await replacing;
 assert.deepEqual(writes.map(j=>j.minutes),[30]);assert.equal(remainingSleepMinutes(store.getSnapshot(),now),30);
 const cancel=store.change(null);writes[1].resolve({revision:4,phase:'idle',remaining_ms:0});await cancel;event({revision:3,phase:'armed',remaining_ms:1000000});assert.equal(store.getSnapshot().timer.phase,'idle');
 const failed=store.change(15);writes[2].reject(Error('unavailable'));await failed;assert.equal(store.getSnapshot().error,true);assert.equal(store.getSnapshot().timer.phase,'idle');
 event({revision:5,phase:'failed',remaining_ms:0});assert.equal(store.getSnapshot().timer.phase,'failed');
 off1();assert.equal(unlisten,0);off2();assert.equal(unlisten,1);
 for(const value of [null,{revision:-1,phase:'idle',remaining_ms:0},{revision:1,phase:'invented',remaining_ms:0},{revision:1,phase:'armed',remaining_ms:4000000},{revision:1,phase:'idle',remaining_ms:1}])assert.throws(()=>decodeSleepTimer(value));
});

test('timer listener registration completing after unmount is disposed and cannot start native polling',async()=>{
 let read=0,cleanup=0;const registration=deferred();const store=new SleepTimerStore({listen:()=>registration.promise,read:async()=>{read++;return{}},set:async()=>({})},()=>0);
 const off=store.subscribe(()=>{});off();registration.resolve(()=>cleanup++);await flush();assert.equal(cleanup,1);assert.equal(read,0);
});

test('actual timer controls expose options, direct cancellation, failure and cleanup without transport side effects',async()=>{
 const oldWindow=globalThis.window,oldDocument=globalThis.document;const intervals=new Map(),listeners=new Map(),changes=[];let id=0;
 try {
  globalThis.window={setInterval:fn=>{intervals.set(++id,fn);return id},clearInterval:id=>intervals.delete(id)};
  globalThis.document={hidden:false,addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:name=>listeners.delete(name)};
  const i18n=i18next.createInstance();await i18n.init({lng:'pt-BR',resources:{'pt-BR':{translation:JSON.parse(readFileSync(new URL('../src/locales/pt-BR.json',import.meta.url),'utf8'))}}});
  const h=hooks();let view={timer:{revision:1,phase:'armed',remaining_ms:900000},receivedAt:performance.now(),error:false,changing:false};
  const {SleepTimerControl}=compile('components/SleepTimerControl.tsx',{
   react:{...h.react,useSyncExternalStore:()=>view},'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../services/sleepTimer':{sleepTimer:{subscribe(){},getSnapshot:()=>view,change:value=>changes.push(value),refresh:async()=>{}}},'../utils/sleepTimer':{remainingSleepMinutes},'./sleepTimer.css':{}
  });
  const render=()=>h.render(()=>SleepTimerControl({}));let tree=render();assert.match(text(tree),/15 min/);assert.equal(intervals.size,1);
  const buttons=nodes(tree,n=>n.type==='button');for(const count of [15,30,60])buttons.find(n=>text(n)===`Parar em ${count} minutos`).props.onClick();
  buttons.find(n=>n.props['aria-label']==='Cancelar temporizador').props.onClick();assert.deepEqual(changes,[15,30,60,null]);
  view={...view,timer:{revision:2,phase:'failed',remaining_ms:0}};tree=render();assert.equal(intervals.size,0);assert.equal(nodes(tree,n=>n.props?.role==='alert').length,1);
  h.dispose();assert.equal(listeners.size,0);assert.equal(intervals.size,0);
 } finally {globalThis.window=oldWindow;globalThis.document=oldDocument}
});


test('lyrics timeout cancels a stalled fetch and does not persist a false missing result',async()=>{
 const oldSet=globalThis.setTimeout,oldClear=globalThis.clearTimeout;const timers=new Map(),saved=storage();let id=0;
 try{
  globalThis.setTimeout=(fn,ms)=>{timers.set(++id,{fn,ms});return id};globalThis.clearTimeout=id=>timers.delete(id);
  const client=new lyrics.LyricsClient((_url,{signal})=>new Promise((_resolve,reject)=>signal.addEventListener('abort',()=>reject(new DOMException('Timed out','AbortError')),{once:true})),()=>saved,()=>1000);
  const result=client.lookup(query(),new AbortController().signal);await flush();assert.equal(timers.size,1);const [key,timer]=[...timers][0];assert.equal(timer.ms,10000);timers.delete(key);timer.fn();
  await assert.rejects(result,/LYRICS_UNAVAILABLE/);await flush();assert.equal(timers.size,0);assert.equal(saved.data.has(lyrics.LYRICS_CACHE_KEY),false);
 }finally{globalThis.setTimeout=oldSet;globalThis.clearTimeout=oldClear}
});


test('observed provider502 honors Retry-After and remains service failure rather than a cached missing track',async()=>{
 let now=100000,calls=0;const saved=storage();let mode='error';
 const client=new lyrics.LyricsClient(async()=>{calls++;return mode==='error'?response({},502,{'Retry-After':'60'}):response(record())},()=>saved,()=>now);
 await assert.rejects(client.lookup(query(),new AbortController().signal),error=>error.reason==='service'&&error.status===502);
 assert.equal(saved.data.has(lyrics.LYRICS_CACHE_KEY),false);
 now+=59000;await assert.rejects(client.lookup(query('Other Song'),new AbortController().signal),error=>error.reason==='service');assert.equal(calls,1,'no retry against an unavailable provider during its cooldown');
 now+=2001;mode='ok';assert.equal((await client.lookup(query(),new AbortController().signal)).kind,'found');assert.equal(calls,2);
});

test('connection/CORS, timeout and malformed response failures have distinct safe reasons',async()=>{
 const client=new lyrics.LyricsClient(async()=>{throw TypeError('Private URL must never become UI text')},()=>null,()=>100000);
 await assert.rejects(client.lookup(query(),new AbortController().signal),error=>error.reason==='connection'&&!error.message.includes('Private'));
 assert.equal(lyrics.lyricsFailureReason(new lyrics.LyricsLookupError('timeout')),'timeout');
 const malformed=new lyrics.LyricsClient(async()=>new Response('invalid JSON',{headers:{'Content-Type':'application/json'}}),()=>null,()=>100000);
 await assert.rejects(malformed.lookup(query(),new AbortController().signal),error=>error.reason==='invalid_response');
});
