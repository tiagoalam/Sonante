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
const record = (q=query(),extra={}) => ({id:1,trackName:q.title,artistName:q.artist,albumName:q.album,duration:q.duration??120,instrumental:false,hasWordSync:false,plainLyrics:'Synthetic first line\nSynthetic second line',syncedLyrics:null,...extra});
const records = (...values) => ({kind:'records',records:values});
const storage = () => {const data=new Map();return{data,getItem:key=>data.get(key)??null,setItem:(key,value)=>data.set(key,value),removeItem:key=>data.delete(key)}};
function compile(file,deps) { const result={};new Function('require','exports',ts.transpileModule(readFileSync(new URL('../src/'+file,import.meta.url),'utf8'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},result);return result; }
function hooks() { let cursor=0;const slots=[],effects=[];return {react:{...React,default:React,useState(initial){const n=cursor++;if(!(n in slots))slots[n]=typeof initial==='function'?initial():initial;return [slots[n],value=>{slots[n]=typeof value==='function'?value(slots[n]):value}]},useRef(initial){return slots[cursor++]??={current:initial}},useEffect(fn,deps){const n=cursor++,previous=slots[n];if(!previous||deps.some((value,i)=>!Object.is(value,previous.deps[i])))effects.push(()=>{previous?.cleanup?.();slots[n]={deps,cleanup:fn()}})}},render(fn){cursor=0;const result=fn();effects.splice(0).forEach(fn=>fn());return result},dispose(){slots.forEach(slot=>slot?.cleanup?.())}}; }
function nodes(tree,predicate,result=[]) {if(!tree||typeof tree!=='object')return result;if(Array.isArray(tree)){tree.forEach(child=>nodes(child,predicate,result));return result}if(predicate(tree))result.push(tree);nodes(tree.props?.children,predicate,result);return result;}
function text(tree) { if(tree===null||tree===undefined||typeof tree==='boolean')return '';if(typeof tree==='string'||typeof tree==='number')return String(tree);if(Array.isArray(tree))return tree.map(text).join(' ');return text(tree.props?.children); }

test('lyrics queries accept identified Local/Plex media without exporting source identifiers and rank recordings',()=>{
 assert.equal(lyrics.lyricsQuery({title:'Live',artist:'Station',current_media:{kind:'radio',station_id:'x',queue_index:0}}),null);
 assert.deepEqual(lyrics.lyricsQuery({...query(),current_media:{kind:'plex',server_id:'private-server',part_key:'private-part',queue_index:1}}),query());
 assert.equal(lyrics.lyricsQuery({...query(),current_media:{kind:'plex',server_id:'private-server',part_key:'private-part'}}),null);
 assert.deepEqual(lyrics.lyricsQuery({...query(),current_media:{kind:'local',uri:'sqlite-cue://private',queue_index:1}}),query());
 for(const title of ['/personal/file.flac','https://private.invalid/?token=SECRET','file:///path','sqlite-track://source/secret','C:\\Private\\file.flac','\\\\server\\share\\track.flac','sensitive token=SECRET','bad\nname'])assert.equal(lyrics.lyricsQuery({...query(),title}),null);
 const value=lyrics.lyricsQuery({...query(),uri:'/personal/path',token:'SECRET',elapsed:50,current_media:{kind:'local',uri:'sqlite-track://private',queue_index:0}});
 assert.deepEqual(value,query());assert.deepEqual(Object.keys(value),['title','artist','album','duration']);
 const exact=record(query(),{syncedLyrics:'[00:01.00]Live first'});
 assert.deepEqual(lyrics.automaticLyricsMatches([exact],query()),[exact]);
 const editorial=record(query(),{id:5,albumName:'Synthetic Album (Super Deluxe Edition) (Remastered)',duration:120.7,syncedLyrics:'[00:01.00]Editorial release'});
 assert.deepEqual(lyrics.automaticLyricsMatches([editorial],query()),[editorial]);
 const duplicateA=record(query(),{id:6,syncedLyrics:'[00:01.00]same'}), duplicateB={...duplicateA,id:7};
 assert.deepEqual(lyrics.automaticLyricsMatches([duplicateA,duplicateB],query()),[duplicateA]);
 const close=record(query(),{id:8,duration:120.5,syncedLyrics:'[00:01.00]different'});
 assert.deepEqual(lyrics.automaticLyricsMatches([close,exact],query()),[exact,close], 'near-tied eligible results are deterministically ranked for immediate adoption');
 const clearlyCloser=record(query(),{id:9,duration:122,syncedLyrics:'[00:01.00]other duration'});
 assert.deepEqual(lyrics.automaticLyricsMatches([clearlyCloser,exact],query()),[exact,clearlyCloser]);
 for(const changed of [record(query(),{artistName:'Other'}),record(query(),{trackName:'Synthetic Song (Live)'}),record(query(),{albumName:'Live Album'}),record(query(),{duration:123})])
  assert.deepEqual(lyrics.automaticLyricsMatches([changed],query()),[]);
 assert.deepEqual(lyrics.automaticLyricsMatches([record(query(),{syncedLyrics:'[00:01.00]x'})],{...query(),duration:null}),[]);
 const noAlbum={...query(),album:''};
 const plainA=record(noAlbum,{id:10,albumName:'Z Album',syncedLyrics:null,plainLyrics:'plain'});
 const syncedA=record(noAlbum,{id:11,albumName:'A Album',plainLyrics:null,syncedLyrics:'[00:01.00]sync'});
 assert.deepEqual(lyrics.automaticLyricsMatches([plainA,syncedA],noAlbum),[syncedA,plainA], 'without album information, synchronized content wins a compatible tie');
 assert.deepEqual(lyrics.automaticLyricsMatches([record(query(),{id:12,albumName:'Live at Venue'})],query()),[], 'live/recording qualifier conflicts are rejected');
 assert.deepEqual(lyrics.automaticLyricsMatches([record(query(),{albumName:'Synthetic Album (Live)'})],query()),[]);
 assert.equal(lyrics.lyricsKey({...query(),title:' SYNTHETIC   Song '}),lyrics.lyricsKey(query()));
 assert.notEqual(lyrics.lyricsKey(query()),lyrics.lyricsKey({...query(),duration:121}));
});

test('backend-shaped records are validated and preserve lyric availability distinctions',async()=>{
 const client=new lyrics.LyricsClient(async()=>records(record(),record(query('Plain'),{id:2,syncedLyrics:null}),record(query('Instrumental'),{id:3,instrumental:true,plainLyrics:null})),()=>null);
 const result=await client.lookup(query(),new AbortController().signal);assert.equal(result.kind,'records');assert.equal(result.records.length,3);
 for(const invalid of [records({...record(),id:0}),records(record(),record(query(),{id:1})),records(...Array.from({length:21},(_,i)=>record(query(`S${i}`),{id:i+1})))]){
  const malformed=new lyrics.LyricsClient(async()=>invalid,()=>null);
  await assert.rejects(malformed.lookup(query(),new AbortController().signal),error=>error.reason==='invalid_response');
 }
});

test('new lyric results are delegated to durable backend storage, not accumulated or expired in localStorage',async()=>{
 let now=1_000_000,calls=0;const saved=storage();const client=new lyrics.LyricsClient(async q=>{calls++;assert.deepEqual(Object.keys(q),['title','artist','album','duration']);return {...records(record(q)),persisted:true}},()=>saved,()=>now);
 const result=await client.lookup(query(),new AbortController().signal);assert.equal(result.kind,'records');
 assert.equal(saved.data.has(lyrics.LYRICS_CACHE_KEY),false,'the WebView does not accumulate new lyric content');
 now+=31*86400_000;
 const later=await client.lookup(query(),new AbortController().signal);assert.deepEqual(later,result);assert.equal(calls,2,'durable result reuse and age policy belong to the backend');
 assert.equal(saved.data.has(lyrics.LYRICS_CACHE_KEY),false);
});

test('legacy positive cache migrates before its copy is removed and remains usable offline when migration fails',async()=>{
 const saved=storage(),oldPositive={key:lyrics.lyricsKey(query()),at:1,result:records(record())};
 saved.setItem(lyrics.LYRICS_CACHE_KEY,JSON.stringify([oldPositive]));let imported=[];
 const migrated=new lyrics.LyricsClient(async()=>{throw Error('network must not be needed')},()=>saved,()=>100_000_000,async entries=>{imported=entries;return true});
 assert.deepEqual(await migrated.lookup(query(),new AbortController().signal),oldPositive.result);
 await flush();assert.equal(imported.length,1);assert.equal(saved.getItem(lyrics.LYRICS_CACHE_KEY),null,'old storage is removed only after backend confirms migration');

 const failedStorage=storage();failedStorage.setItem(lyrics.LYRICS_CACHE_KEY,JSON.stringify([oldPositive]));let requests=0;
 const retained=new lyrics.LyricsClient(async()=>{requests++;throw Error('offline')},()=>failedStorage,()=>100_000_000,async()=>false);
 assert.deepEqual(await retained.lookup(query(),new AbortController().signal),oldPositive.result);
 await flush();assert.ok(failedStorage.getItem(lyrics.LYRICS_CACHE_KEY),'failed migration retains the recoverable old copy');
 assert.equal(requests,0,'valid legacy positive content remains available offline');

 const manyStorage=storage(),many=Array.from({length:25},(_,index)=>{const q=query(`Legacy ${index}`);return{key:lyrics.lyricsKey(q),at:1,result:records(record(q,{id:index+1}))}});
 manyStorage.setItem(lyrics.LYRICS_CACHE_KEY,JSON.stringify(many));let importedCount=0;
 const manyClient=new lyrics.LyricsClient(async()=>{throw Error('legacy positive should remain available')},()=>manyStorage,()=>100_000_000,async entries=>{importedCount=entries.length;return true});
 await manyClient.lookup(query('Legacy 24'),new AbortController().signal);await flush();
 assert.equal(importedCount,25,'migration no longer applies the former twenty-entry cache ceiling');
 assert.equal(manyStorage.getItem(lyrics.LYRICS_CACHE_KEY),null);
});

test('explicit refresh bypasses a legacy hit without deleting it before a successful update',async()=>{
 const saved=storage(),oldPositive={key:lyrics.lyricsKey(query()),at:1,result:records(record(query(),{plainLyrics:'old valid lyric'}))};
 saved.setItem(lyrics.LYRICS_CACHE_KEY,JSON.stringify([oldPositive]));const refreshes=[];
 const client=new lyrics.LyricsClient(async(_q,_signal,refresh)=>{refreshes.push(refresh);return {kind:'offline'}},()=>saved,()=>100_000_000,async()=>false);
 await assert.rejects(client.lookup(query(),new AbortController().signal,true),error=>error.reason==='offline');
 assert.deepEqual(refreshes,[true]);assert.ok(saved.getItem(lyrics.LYRICS_CACHE_KEY),'failed refresh preserves the prior valid copy');
});

test('lyrics deduplicates consumers and honors cancellation of abandoned requests',async()=>{
 const jobs=[];const saved=storage();const client=new lyrics.LyricsClient((q,signal)=>{
  const job={...deferred(),q,signal};jobs.push(job);signal.addEventListener('abort',()=>job.reject(new DOMException('Canceled','AbortError')),{once:true});return job.promise;
 },()=>saved);
 const first=new AbortController(),second=new AbortController(),queued=new AbortController();
 const p1=client.lookup(query(),first.signal),p2=client.lookup(query(),second.signal);await flush();assert.equal(jobs.length,1);
 first.abort();await assert.rejects(p1,{name:'AbortError'});assert.equal(jobs[0].signal.aborted,false,'another consumer retains the in-flight request');
 const abandoned=client.lookup(query('Abandoned'),queued.signal);queued.abort();await assert.rejects(abandoned,{name:'AbortError'});
 jobs[0].resolve(records(record()));assert.equal((await p2).kind,'records');await flush();assert.equal(jobs.length,1,'aborted queued metadata never goes online');
 const last=new AbortController(),p3=client.lookup(query('New Song'),last.signal);await flush();assert.equal(jobs.length,2);last.abort();await assert.rejects(p3,{name:'AbortError'});assert.ok(jobs[1].signal.aborted);await flush();
 assert.equal(saved.data.has(lyrics.LYRICS_CACHE_KEY),false,'in-flight request metadata is never persisted by the WebView client');
});

test('backend failures are distinct and are never cached as a missing record',async()=>{
 let calls=0,mode='not_found';const saved=storage();const client=new lyrics.LyricsClient(async()=>{calls++;return {kind:mode}},()=>saved);
 assert.deepEqual(await client.lookup(query(),new AbortController().signal),{kind:'not_found'});await client.lookup(query(),new AbortController().signal);assert.equal(calls,2,'negative caching is owned by the backend store');
 for(const [backend,reason] of [['offline','offline'],['timeout','timeout'],['rate_limited','rate_limited'],['service_unavailable','service'],['invalid_response','invalid_response']]){
  mode=backend;await assert.rejects(client.lookup(query(backend),new AbortController().signal),error=>error.reason===reason);
 }
 assert.equal(saved.data.has(lyrics.LYRICS_CACHE_KEY),false,'negative results are stored only by the backend policy');
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
  const render=(compact=false)=>h.render(()=>SleepTimerControl({compact}));let tree=render();assert.match(text(tree),/15 min/);assert.equal(intervals.size,1);
  const compactTree=render(true),summary=nodes(compactTree,n=>n.type==='summary')[0];assert.match(text(compactTree),/15m/);assert.match(summary.props.className,/sleep-timer-summary-active/);
  const timerCss=readFileSync(new URL('../src/components/sleepTimer.css',import.meta.url),'utf8');assert.match(timerCss,/sleep-timer-compact summary \{[^}]*width: 36px; height: 36px;[^}]*border-color/);
  const buttons=nodes(tree,n=>n.type==='button');for(const count of [15,30,60])buttons.find(n=>text(n)===`Parar em ${count} minutos`).props.onClick();
  buttons.find(n=>n.props['aria-label']==='Cancelar temporizador').props.onClick();assert.deepEqual(changes,[15,30,60,null]);
  view={...view,timer:{revision:2,phase:'failed',remaining_ms:0}};tree=render();assert.equal(intervals.size,0);assert.equal(nodes(tree,n=>n.props?.role==='alert').length,1);
  h.dispose();assert.equal(listeners.size,0);assert.equal(intervals.size,0);
 } finally {globalThis.window=oldWindow;globalThis.document=oldDocument}
});


test('backend outcomes cover 404, 429, timeout, 5xx, offline and invalid HTTP 200 shapes',async()=>{
 for(const [kind,reason] of [['not_found',null],['rate_limited','rate_limited'],['timeout','timeout'],['service_unavailable','service'],['offline','offline'],['invalid_response','invalid_response']]){
  const client=new lyrics.LyricsClient(async()=>({kind}),()=>null);
  if(reason) await assert.rejects(client.lookup(query(kind),new AbortController().signal),error=>error.reason===reason);
  else assert.deepEqual(await client.lookup(query(kind),new AbortController().signal),{kind:'not_found'});
 }
 const invalid200=new lyrics.LyricsClient(async()=>({kind:'records',records:[{...record(),duration:NaN}]}),()=>null);
 await assert.rejects(invalid200.lookup(query(),new AbortController().signal),error=>error.reason==='invalid_response');
 assert.equal(lyrics.lyricsFailureReason(new lyrics.LyricsLookupError('timeout')),'timeout');
});
