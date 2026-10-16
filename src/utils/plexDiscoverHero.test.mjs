import test from "node:test";
import assert from "node:assert/strict";
import { PlexDiscoverSession, mergeHeroCandidates, recentShelfAlbumKeys, discoverAlbumKey, DISCOVER_HERO_WINDOW } from "./plexDiscover.ts";
import { discoverItemKey } from "./plexDiscoverPresentation.ts";
const flush=async()=>{for(let i=0;i<100;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
const library=key=>({key,title:`Source ${key}`});
const scope=(id="7",serverId="a",libraries=[library("7")])=>({identity:{},serverId,libraryId:id,language:"en",libraries});
const item=(id="12",library_id="7",server_id="a",added_at=100)=>({server_id,library_id,kind:"album",rating_key:id,title:"Homonymous album",subtitle:"Artist",thumb:null,album:{rating_key:id,title:"Homonymous album",artist:"Artist"},added_at});
const page=(s,id,items=[])=>({language:s.language,server_id:s.serverId,library_id:id,partial:false,hubs:[{id:"unrelated-recommendation",title:"Not a recent source",items,partial:false}]});
const recent=(s,id,items)=>({language:s.language,server_id:s.serverId,library_id:id,items,partial:false,dates_missing:false});
const session=(s,read,random=()=>0)=>new PlexDiscoverSession(async(_,id)=>page(s,id,[item('999',id,s.serverId,9999)]),async()=>assert.fail('no artist/details lookup'),read,random);

test("hero uses its actual 20-album query pool, is stable, renews explicitly and preserves shelves/origin",async()=>{
 const s=scope();let draws=0,reads=0;const pool=Array.from({length:20},(_,i)=>item(String(100+i),'7','a',1000-i));
 const p=session(s,async(server,id,language)=>{reads++;assert.deepEqual([server,id,language],['a','7','en']);return recent(s,id,pool);},()=>{draws++;return .999;});p.setContext(s);p.ensure();await flush();
 let hero=p.getSnapshot().hero;assert.equal(hero.candidates.length,20);assert.equal(hero.item.rating_key,'119');assert.notEqual(hero.item.rating_key,'999','never substitutes a recommendation');assert.equal(draws,1);assert.equal(reads,1);
 const chosen=hero.item,rows=p.getSnapshot().libraries[0].page.hubs;p.position=510;p.saveShelfPosition(rows[0].id,210);
 assert.deepEqual(p.album(chosen),{...chosen.album,server_id:'a',library_id:'7',library_title:'Source 7'});p.ensure();p.getSnapshot();p.subscribe(()=>{})();p.back();p.retry();await flush();
 p.setContext({...s,libraries:[{key:'7',title:'Renamed label'},library('8')]});assert.equal(p.getSnapshot().hero.item,chosen);assert.equal(draws,1);assert.equal(reads,1,'unrelated recommendation refresh does not refetch/redraw hero');assert.equal(p.position,510);assert.equal(p.shelfPosition(rows[0].id),210);
 const refreshedRows=p.getSnapshot().libraries[0].page.hubs;p.anotherDiscovery();hero=p.getSnapshot().hero;assert.notEqual(discoverItemKey(hero.item),discoverItemKey(chosen));assert.equal(hero.item.rating_key,'118');assert.equal(draws,2);assert.equal(p.getSnapshot().libraries[0].page.hubs,refreshedRows,'hero only');
});

test("empty, singleton and small pools have no artificial fallback or repeated-current draw",async()=>{
 for(const count of [0,1,2,7]){
  const s=scope(),pool=Array.from({length:count},(_,i)=>item(String(100+i)));let draws=0;
  const p=session(s,async(_,id)=>recent(s,id,pool),()=>{draws++;return 0;});p.setContext(s);p.ensure();await flush();
  assert.equal(p.getSnapshot().hero.phase,'ready');assert.equal(p.getSnapshot().hero.candidates.length,count);assert.equal(!!p.getSnapshot().hero.item,count>0);const before=p.getSnapshot().hero.item,firstDraws=draws;
  p.anotherDiscovery();if(count<2){assert.equal(p.getSnapshot().hero.item,before);assert.equal(draws,firstDraws);}else assert.notEqual(p.getSnapshot().hero.item,before);
 }
});

test("All waits for the complete cohort, ranks by addedAt and is independent of response order",async()=>{
 const libs=[library('1'),library('2'),library('3')];const keys=[];
 for(const reverse of [false,true]){
  const s=scope('all','a',libs),pending=[];let draws=0;
  const p=session(s,(_,id)=>{const d=deferred();pending.push({id,d});return d.promise;},()=>{draws++;return .5;});p.setContext(s);p.ensure();await flush();
  const reply=id=>recent(s,id,Array.from({length:20},(_,i)=>item(String(100+i),id,'a',Number(id)*1000-i)));
  while(pending.length<3){const available=pending.filter(x=>!x.done);const r=available[reverse?available.length-1:0];r.done=true;r.d.resolve(reply(r.id));await flush();assert.equal(draws,0);assert.equal(p.getSnapshot().hero.item,null);}
  const rest=pending.filter(x=>!x.done);if(reverse)rest.reverse();
  for(let i=0;i<rest.length;i++){rest[i].d.resolve(reply(rest[i].id));await flush();if(i<rest.length-1){assert.equal(draws,0);assert.equal(p.getSnapshot().hero.item,null);}}
  const hero=p.getSnapshot().hero;assert.equal(draws,1);assert.equal(hero.candidates.length,20);assert.ok(hero.candidates.every(i=>i.library_id==='3'),'globally newest source, not first reply');assert.equal(hero.item.library_id,'3');keys.push(hero.candidates.map(discoverItemKey));
 }
 assert.deepEqual(keys[0],keys[1]);
});

test("All preserves partial availability, excludes unrankable dates and retries only the failed recent source without redraw",async()=>{
 const s=scope('all','a',[library('1'),library('2'),library('3')]);let fail=true,draws=0;const reads=[];
 const p=session(s,async(_,id)=>{reads.push(id);if(id==='2'&&fail)throw 'PLEX_DISCOVER_AUTH';return {...recent(s,id,[item('12',id,'a',Number(id)*100),item('13',id,'a',null)]),dates_missing:true};},()=>{draws++;return 0;});p.setContext(s);p.ensure();await flush();
 const before=p.getSnapshot().hero;assert.equal(before.phase,'ready');assert.equal(before.partial,true);assert.equal(before.datesMissing,true);assert.equal(before.candidates.length,2);assert.equal(before.selectionCount,2);assert.equal(p.getSnapshot().libraries[1].recentError,'PLEX_DISCOVER_AUTH');
 assert.equal(p.album(before.item).library_id,'3');fail=false;p.retryHero('2');await flush();const after=p.getSnapshot().hero;assert.equal(after.item,before.item);assert.equal(draws,1);assert.equal(after.selectionCount,2);assert.equal(after.selectionPartial,true);assert.deepEqual(reads,['1','2','3','2']);assert.equal(after.candidates.length,3);
 p.anotherDiscovery();assert.equal(p.getSnapshot().hero.selectionCount,3);assert.notEqual(p.getSnapshot().hero.item,before.item);
});

test("All homonymous albums retain source identity and missing dates never borrow release/play dates",()=>{
 const a=item('12','1','a',300),b=item('12','2','a',400),unknown={...item('13','2','a',null),last_viewed_at:999999,year:2026};
 const pool=mergeHeroCandidates([], [a,b,unknown],true);assert.equal(pool.length,2);assert.equal(pool[0],b);assert.notEqual(discoverItemKey(pool[0]),discoverItemKey(pool[1]));assert.equal(mergeHeroCandidates([], [unknown],false).length,1,'individual trusts requested server sort');
});

test("rapid selection/server/account/language changes reject obsolete recent replies and share actual IPC slots",async()=>{
 const calls=[];let active=0,max=0;
 const track=async(fn)=>{active++;max=Math.max(max,active);try{return await fn();}finally{active--;}};
 let s=scope('all','a',[library('1'),library('2'),library('3')]);
 const p=new PlexDiscoverSession(async(_,id)=>track(async()=>page(s,id)),async()=>assert.fail(),(server,id,language)=>track(()=>{const d=deferred();calls.push({server,id,language,d});return d.promise;}),()=>0);
 p.setContext(s);p.ensure();await flush();assert.equal(calls.length,2);
 const stale=s;s={...scope('7','b'),language:'pt-BR'};p.setContext(s);p.ensure();assert.equal(p.getSnapshot().hero.item,null);assert.equal(calls.length,2,'obsolete IPCs keep actual slots');
 for(const c of calls)c.d.resolve(recent(stale,c.id,[item('12',c.id,'a',100)]));await flush();assert.equal(calls.length,3);assert.equal(p.getSnapshot().hero.item,null);
 const latest=calls[2];latest.d.resolve(recent(s,'7',[item('88','7','b',500)]));await flush();assert.equal(p.getSnapshot().hero.item.rating_key,'88');assert.equal(p.getSnapshot().hero.item.server_id,'b');assert.equal(max,2);
 const old=p.getSnapshot().hero.item;p.setContext({...s,identity:{}});assert.equal(p.getSnapshot().hero.item,null);assert.equal(p.album(old),null);
});

test("unavailable/malformed/foreign recent replies never fall back to recommendations",async()=>{
 for(const response of ['PLEX_DISCOVER_UNAVAILABLE',{wrong:'payload'},recent(scope(),'7',[item('12','foreign')]),recent(scope(),'7',[{...item(),kind:'artist'}]),{...recent(scope(),'7',[]),language:'pt-BR'},recent(scope(),'7',Array.from({length:21},(_,i)=>item(String(i))))]){
  const s=scope(),p=session(s,async()=>{if(typeof response==='string')throw response;return response;});p.setContext(s);p.ensure();await flush();assert.equal(p.getSnapshot().hero.phase,'error');assert.equal(p.getSnapshot().hero.item,null);assert.equal(p.getSnapshot().hero.candidates.length,0);assert.equal(p.getSnapshot().libraries[0].page.hubs.length,1);
 }
});

test("same-server credentials and locale invalidate pending hero data even when item/library IDs match",async()=>{
 let s=scope();const calls=[];let draws=0;
 const p=session(s,(_,id,language)=>{const d=deferred();calls.push({id,language,d});return d.promise;},()=>{draws++;return 0;});p.setContext(s);p.ensure();await flush();
 const account={...s,identity:{}};p.setContext(account);p.ensure();await flush();assert.equal(calls.length,2);
 calls[0].d.resolve(recent(s,'7',[item('110')]));await flush();assert.equal(p.getSnapshot().hero.item,null);assert.equal(draws,0);
 calls[1].d.resolve(recent(account,'7',[item('111')]));await flush();assert.equal(p.getSnapshot().hero.item.rating_key,'111');assert.equal(draws,1);
 const localized={...account,language:'pt-BR'};p.setContext(localized);p.ensure();await flush();assert.equal(p.getSnapshot().hero.item,null);assert.equal(calls[2].language,'pt-BR');
 calls[2].d.resolve(recent(localized,'7',[item('112')]));await flush();assert.equal(p.getSnapshot().hero.item.rating_key,'112');assert.equal(draws,2);
});

test("All with only missing dates stays honestly empty, while an explicit retry can recover an unavailable source",async()=>{
 const all=scope('all','a',[library('7')]);let fail=false,missing=true;
 const p=session(all,async(_,id)=>{if(fail)throw 'PLEX_DISCOVER_UNAVAILABLE';return {...recent(all,id,[item('12',id,'a',missing?null:100)]),dates_missing:missing};},()=>0);
 p.setContext(all);p.ensure();await flush();assert.equal(p.getSnapshot().hero.item,null);assert.equal(p.getSnapshot().hero.phase,'ready');assert.equal(p.getSnapshot().hero.partial,true);assert.equal(p.getSnapshot().hero.datesMissing,true);
 const next={...all,identity:{}};fail=true;p.setContext(next);p.ensure();await flush();assert.equal(p.getSnapshot().hero.phase,'error');assert.equal(p.getSnapshot().hero.item,null);
 fail=false;missing=false;p.retryHero('7');await flush();assert.equal(p.getSnapshot().hero.phase,'ready');assert.equal(p.getSnapshot().hero.partial,false);assert.equal(p.getSnapshot().hero.item.rating_key,'12');
});

test("excludes the complete added shelf by identity and requests shelf size plus20, not a fixed skip",async()=>{
 for(const count of [0,1,5,12]){
  const s=scope(),catalog=Array.from({length:60},(_,i)=>item(String(100+i),'7','a',10000-i)),windows=[];
  const p=new PlexDiscoverSession(async()=>({...page(s,'7'),hubs:[{id:'added',recent_family:'added',title:'Localized header',items:catalog.slice(0,count),partial:false}]}),async()=>assert.fail(),async(_,id,lang,window)=>{windows.push(window);return recent(s,id,catalog.slice(0,window));},()=>0);
  p.setContext(s);p.ensure();await flush();const hero=p.getSnapshot().hero;assert.deepEqual(windows,[count+20]);assert.equal(hero.candidates.length,20);assert.equal(hero.item.rating_key,String(100+count));
  const excluded=new Set(p.getSnapshot().libraries[0].page.hubs[0].items.map(discoverItemKey));assert.ok(hero.candidates.every(i=>!excluded.has(discoverItemKey(i))),'includes horizontally offscreen shelf entries');
  const rows=p.getSnapshot().libraries[0].page.hubs;p.anotherDiscovery();assert.equal(p.getSnapshot().libraries[0].page.hubs,rows);assert.notEqual(p.getSnapshot().hero.item,hero.item);
 }
});

test("complete overlap is honestly empty, partial overlap can remain smaller than20, and names never identify shelves",async()=>{
 for(const available of [5,8]){
  const s=scope(),catalog=Array.from({length:available},(_,i)=>item(String(100+i),'7','a',10000-i));
  const p=new PlexDiscoverSession(async()=>({...page(s,'7'),hubs:[{id:'added',recent_family:'added',title:'Anything',items:catalog.slice(0,5),partial:false}]}),async()=>assert.fail(),async(_,id)=>recent(s,id,catalog),()=>0);p.setContext(s);p.ensure();await flush();assert.equal(p.getSnapshot().hero.candidates.length,available-5);assert.equal(p.getSnapshot().hero.item?.rating_key??null,available===5?null:'105');p.anotherDiscovery();assert.equal(p.getSnapshot().hero.item===null,available===5);
 }
 const s=scope(),candidate=item('12');
 const p=new PlexDiscoverSession(async()=>({...page(s,'7'),hubs:[{id:'custom.collection.12',title:'Adicionados recentemente',items:[candidate],partial:false}]}),async()=>assert.fail(),async(_,id)=>recent(s,id,[candidate]),()=>0);p.setContext(s);p.ensure();await flush();assert.equal(p.getSnapshot().hero.item.rating_key,'12','custom title is not the built-in added family');
});

test("All exclusion uses only the aggregate shelf and not an arbitrary per-library offset",async()=>{
 const s=scope('all','a',[library('1'),library('2'),library('3')]),root=[],queries=[];
 const catalog=id=>Array.from({length:id==='3'?25:15},(_,i)=>item(String(100+i),id,'a',(4-Number(id))*1000-i));
 const p=new PlexDiscoverSession((_,id)=>{const d=deferred();root.push({id,d});return d.promise;},async()=>assert.fail(),(_,id,lang,window)=>{const d=deferred();queries.push({id,window,d});return d.promise;},()=>.8);
 let duplicates=0;p.subscribe(()=>{const state=p.getSnapshot(),keys=new Set(state.hubs.filter(h=>h.recent_family==='added').flatMap(h=>h.items.map(discoverItemKey)));if(state.hero.item&&keys.has(discoverItemKey(state.hero.item)))duplicates++;});
 p.setContext(s);p.ensure();assert.equal(queries.length,0);root[1].d.resolve({...page(s,'2'),hubs:[{id:'added',recent_family:'added',title:'Added',items:catalog('2').slice(0,12),partial:false}]});await flush();assert.equal(queries.length,0,'late root can still change the exclusion set');
 root.find(r=>r.id==='3').d.resolve({...page(s,'3'),hubs:[{id:'added',recent_family:'added',title:'Added',items:catalog('3').slice(0,12),partial:false}]});await flush();assert.equal(queries.length,0);
 root[0].d.resolve({...page(s,'1'),hubs:[{id:'added',recent_family:'added',title:'Added',items:catalog('1').slice(0,12),partial:false}]});await flush();assert.equal(queries.length,2);assert.ok(queries.every(q=>q.window===44));
 queries.find(q=>q.id==='2').d.resolve(recent(s,'2',catalog('2')));await flush();assert.equal(p.getSnapshot().hero.item,null);queries.find(q=>q.id==='3').d.resolve(recent(s,'3',catalog('3')));await flush();assert.equal(p.getSnapshot().hero.item,null);queries.find(q=>q.id==='1').d.resolve(recent(s,'1',catalog('1')));await flush();
 const hero=p.getSnapshot().hero;assert.equal(hero.candidates.length,20);assert.deepEqual(hero.candidates.map(i=>[i.library_id,i.rating_key]),[...Array.from({length:3},(_,i)=>['1',String(112+i)]),...Array.from({length:3},(_,i)=>['2',String(112+i)]),...Array.from({length:14},(_,i)=>['3',String(100+i)])]);assert.equal(duplicates,0);assert.equal(p.album(hero.item).library_id,'3');
});

test("homonymous IDs in another library remain eligible and the explicit window covers multiple shelves",async()=>{
 const s=scope('all','a',[library('1'),library('2')]),a=item('12','1','a',100),b=item('12','2','a',90);
 const p=new PlexDiscoverSession(async(_,id)=>({...page(s,id),hubs:id==='1'?[{id:'added',recent_family:'added',title:'Added',items:[a],partial:false}]:[]}),async()=>assert.fail(),async(_,id)=>recent(s,id,[id==='1'?a:b]),()=>0);p.setContext(s);p.ensure();await flush();assert.equal(p.getSnapshot().hero.item.library_id,'2');assert.equal(p.getSnapshot().hero.candidates.length,1);
 const single=scope(),catalog=Array.from({length:116},(_,i)=>item(String(100+i),'7','a',10000-i));let requested=0;
 const large=new PlexDiscoverSession(async()=>({...page(single,'7'),hubs:Array.from({length:8},(_,i)=>({id:`added-${i}`,recent_family:'added',title:'Added',items:catalog.slice(i*12,(i+1)*12),partial:false}))}),async()=>assert.fail(),async(_,id,lang,window)=>{requested=window;return recent(single,id,catalog.slice(0,window));},()=>0);large.setContext(single);large.ensure();await flush();assert.equal(requested,116);assert.equal(large.getSnapshot().hero.candidates.length,20);assert.equal(large.getSnapshot().hero.item.rating_key,'196');
});

test("explicit shelf refresh revalidates atomically while valid chosen albums, position and shelf order stay stable",async()=>{
 const s=scope(),catalog=Array.from({length:60},(_,i)=>item(String(100+i),'7','a',10000-i));let count=1,reads=0,draws=0;
 const p=new PlexDiscoverSession(async()=>({...page(s,'7'),hubs:[{id:'added',recent_family:'added',title:'Added',items:catalog.slice(0,count),partial:false}]}),async()=>assert.fail(),async(_,id,lang,window)=>{reads++;return recent(s,id,catalog.slice(0,window));},()=>{draws++;return 0;});
 let duplicate=false;p.subscribe(()=>{const state=p.getSnapshot();if(state.hero.item&&state.libraries[0]?.page?.hubs.some(h=>h.items.some(i=>discoverItemKey(i)===discoverItemKey(state.hero.item))))duplicate=true;});p.setContext(s);p.ensure();await flush();assert.equal(p.getSnapshot().hero.item.rating_key,'101');p.position=650;const shelfId=p.getSnapshot().libraries[0].page.hubs[0].id;p.saveShelfPosition(shelfId,300);
 count=2;p.retry();await flush();assert.equal(p.getSnapshot().hero.item.rating_key,'102');assert.equal(draws,2);assert.equal(reads,2);assert.equal(duplicate,false);assert.deepEqual(p.getSnapshot().libraries[0].page.hubs[0].items.map(i=>i.rating_key),['100','101']);assert.equal(p.position,650);assert.equal(p.shelfPosition(shelfId),300);
 const chosen=p.getSnapshot().hero.item;count=1;p.retry();await flush();assert.equal(p.getSnapshot().hero.item,chosen,'still eligible choice survives a changed pool');assert.equal(draws,2);assert.equal(reads,3);
 p.retry();await flush();assert.equal(reads,3,'unchanged exclusion does not repeat the recent query');assert.equal(p.getSnapshot().hero.item,chosen);
});

test("a late candidate response from before explicit shelf refresh cannot repopulate the hero",async()=>{
 const s=scope(),catalog=Array.from({length:60},(_,i)=>item(String(100+i),'7','a',10000-i)),roots=[],queries=[];
 const p=new PlexDiscoverSession(()=>{const d=deferred();roots.push(d);return d.promise;},async()=>assert.fail(),(_,id,lang,window)=>{const d=deferred();queries.push({id,window,d});return d.promise;},()=>0);
 const response=n=>({...page(s,'7'),hubs:[{id:'added',recent_family:'added',title:'Added',items:catalog.slice(0,n),partial:false}]});p.setContext(s);p.ensure();roots[0].resolve(response(1));await flush();assert.equal(queries[0].window,21);p.retry();await flush();assert.equal(p.getSnapshot().hero.item,null);
 roots[1].resolve(response(6));await flush();assert.equal(queries[1].window,26);queries[1].d.resolve(recent(s,'7',catalog.slice(0,26)));await flush();const chosen=p.getSnapshot().hero.item;assert.equal(chosen.rating_key,'106');assert.equal(p.getSnapshot().hero.candidates.length,20);
 queries[0].d.resolve(recent(s,'7',catalog.slice(0,21)));await flush();assert.equal(p.getSnapshot().hero.item,chosen);assert.equal(p.getSnapshot().hero.candidates.length,20,'obsolete narrower window cannot truncate the new pool');
});

test("documented shelf identity uses album references and no title comparison, with an explicit bounded window",()=>{
 const album=item('12'),track={...item('999'),kind:'track',album:{...album.album,rating_key:'12'}};
 const builtin={id:'scoped',source_id:'home.music.recent',context:'hub.home.music.recent',title:'Any localized name',items:[track],partial:false};
 const keys=recentShelfAlbumKeys([builtin]);assert.equal(keys.has(discoverAlbumKey(album)),true);assert.equal(keys.has(discoverAlbumKey(item('12','other-library'))),false);assert.equal(keys.has(discoverAlbumKey(item('12','7','other-server'))),false);
 assert.equal(recentShelfAlbumKeys([{...builtin,source_id:'custom.collection',title:'Adicionados recentemente'}]).size,0);assert.equal(DISCOVER_HERO_WINDOW,140);
});
