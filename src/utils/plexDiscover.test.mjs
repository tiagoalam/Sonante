import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { PlexDiscoverSession as DiscoverSession, DISCOVER_RECENT_LIMIT, mergeRecentItems } from "./plexDiscover.ts";
import { discoverHubTitle, discoverLanguage, discoverItemKey } from "./plexDiscoverPresentation.ts";
class PlexDiscoverSession extends DiscoverSession {
 constructor(load, children, recent=async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),random=()=>0,fullArtist) {
  super(load,children,recent,random,fullArtist);
 }
}
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<50;i++)await Promise.resolve();};
const library=key=>({key,title:`Library ${key}`});
const scope=(serverId="a",libraryId="7",language="en",libraries=[library("7")])=>({identity:{},serverId,libraryId,language,libraries});
const item=(rating_key="12",kind="album",library_id="7",server_id="a")=>({server_id,library_id,kind,rating_key,title:"Same title",subtitle:"Artist",thumb:null,album:kind==="album"?{rating_key,title:"Same title",artist:"Artist"}:null});
const page=(s,items=[item("12","album",s.libraryId,s.serverId)],id=s.libraryId)=>({language:s.language,server_id:s.serverId,library_id:id,partial:false,hubs:[{id:"server-row",title:"Server title",partial:false,items}]});
const row=(session,id="7")=>session.getSnapshot().libraries.find(row=>row.library.key===id);
const current=(session,id="7")=>row(session,id).page.hubs[0].items;
// Synthetic raw PMS fixture matches the observed contract; Rust tests validate
// recognition. This adapter models the parsed IPC response, not another parser.
const playedArtistsFixture=JSON.parse(readFileSync(new URL('../../tests/fixtures/plex-discover-recent-played-artists.json',import.meta.url),'utf8'));
const artistPage=(id,language='en',offset=0)=>({server_id:'a',library_id:id,language,partial:false,hubs:playedArtistsFixture.MediaContainer.Hub.map((hub,index)=>({
 id:hub.hubIdentifier,context:hub.context,title:hub.title,partial:false,recent_family:index===0?'played_artists':null,
 items:hub.Metadata.map(entry=>({...item(entry.ratingKey,'artist',id),title:entry.title,subtitle:'',added_at:entry.addedAt,last_viewed_at:entry.lastViewedAt+offset}))
}))});
const artistRecommendationsFixture=JSON.parse(readFileSync(new URL('../../tests/fixtures/plex-discover-artist-recommendations.json',import.meta.url),'utf8'));
test('full artist loading preserves original All origin, rejects foreign results and never uses collection children',async()=>{
 const s=scope('a','all','en',[library('1'),library('2')]),calls=[];
 const full=async(server,id,key,language)=>{calls.push({server,id,key,language});return{server_id:server,library_id:id,artist_id:key,albums:Array.from({length:32},(_,i)=>({...item(String(12+i)).album,artist_rating_key:key})),top_tracks:[]};};
 const session=new PlexDiscoverSession(async(_,id)=>page(s,[item('88','artist',id)],id),()=>assert.fail('no artist child detail'),undefined,()=>0,full);session.setContext(s);session.ensure();await flush();
 const card=session.getSnapshot().hubs.find(h=>h.library_id==='2').items[0],artist=session.artist(card);
 const content=await session.loadArtist(artist);assert.deepEqual(calls,[{server:'a',id:'2',key:'88',language:'en'}]);assert.equal(content.albums.length,32,'full ArtistView is not the capped collection detail');assert.equal(content.albums[0].library_id,'2');assert.equal(content.albums[0].server_id,'a');assert.equal(session.getSnapshot().detail,null);
 for(const result of [{server_id:'other',library_id:'2',artist_id:'88',albums:[],top_tracks:[]},{server_id:'a',library_id:'1',artist_id:'88',albums:[],top_tracks:[]},{server_id:'a',library_id:'2',artist_id:'89',albums:[],top_tracks:[]},{server_id:'a',library_id:'2',artist_id:'88',albums:[{...item().album,artist_rating_key:'wrong'}],top_tracks:[]}]){
  session.fullArtist=async()=>result;await assert.rejects(session.loadArtist(artist),e=>e==='PLEX_DISCOVER_CONTEXT');
 }
});
test('running full-artist replies retain capacity and are rejected after account/context changes; queued requests cancel',async()=>{
 const s=scope(),pending=deferred();
 const session=new PlexDiscoverSession(async()=>page(s,[item('88','artist')]),async()=>assert.fail(),undefined,()=>0,()=>pending.promise);session.setContext(s);session.ensure();await flush();
 const artist=session.artist(current(session)[0]);const content=session.loadArtist(artist);const rejected=assert.rejects(content,e=>e==='PLEX_DISCOVER_CONTEXT');
 session.setContext({...s,identity:{}});assert.equal(session.running,1,'actual running request retains its slot');pending.resolve({server_id:'a',library_id:'7',artist_id:'88',albums:[],top_tracks:[]});await rejected;await flush();assert.equal(session.running,0);
 const loads=[deferred(),deferred()],all=scope('a','all','en',[library('1'),library('2')]);let reads=0;
 const queued=new PlexDiscoverSession(()=>loads[reads++].promise,async()=>assert.fail(),undefined,()=>0,async()=>assert.fail('cancelled queued artist request must not start'));queued.setContext(all);queued.ensure();
 const cancelled=assert.rejects(queued.loadArtist({rating_key:'88',name:'Synthetic',server_id:'a',library_id:'1'}),e=>e==='PLEX_DISCOVER_CONTEXT');queued.cancelArtist();await cancelled;assert.equal(queued.running,2);
 queued.setContext(null);loads[0].resolve(page({...all,libraryId:'1'},[], '1'));loads[1].resolve(page({...all,libraryId:'2'},[],'2'));await flush();assert.equal(queued.running,0);
});
const recommendationsPage=(id='7',language='en')=>({server_id:'a',library_id:id,language,partial:false,hubs:artistRecommendationsFixture.MediaContainer.Hub.map((h,index)=>({
 id:h.hubIdentifier,title:h.title,context:h.context,artist_recommendation:index===0,partial:false,
 items:h.Metadata.map(raw=>({...item(raw.ratingKey,'album',id),album:{...item(raw.ratingKey).album,artist_rating_key:raw.parentRatingKey}})),
 artist_candidates:h.Metadata.map(raw=>({...item(raw.parentRatingKey,'artist',id),title:raw.parentTitle,subtitle:'',thumb:{server_id:'a',path:raw.parentThumb}}))
}))});

test('individual artist suggestions prioritize evidenced parents, deduplicate by source identity and cap at ten without replacing label/genre rows',async()=>{
 const s=scope();let reads=0;
 const response=recommendationsPage();const more={...response.hubs[0],id:'music.recent.artist.second',items:[{...item('104'),album:{...item('104').album,artist_rating_key:'505'}}],artist_candidates:[item('505','artist')]};
 const genre=response.hubs[2];genre.artist_candidates=Array.from({length:12},(_,i)=>({...item(String(502+i),'artist'),title:'Same synthetic artist name'}));
 genre.items=genre.artist_candidates.map((artist,i)=>({...item(String(200+i)),album:{...item(String(200+i)).album,artist_rating_key:artist.rating_key}}));
 response.hubs.push(more);
 const session=new PlexDiscoverSession(async()=>{reads++;return response;},async()=>assert.fail());session.setContext(s);session.ensure();await flush();
 const hubs=session.getSnapshot().hubs,selection=hubs.find(h=>h.presentation==='artist_suggestions');
 assert.equal(selection.items.length,10);assert.deepEqual(selection.items.slice(0,2).map(i=>i.rating_key),['501','505']);
 assert.equal(new Set(selection.items.map(discoverItemKey)).size,10);assert.ok(selection.items.filter(i=>i.title==='Same synthetic artist name').length>1,'homonyms are retained');
 assert.deepEqual(hubs.filter(h=>!h.presentation).map(h=>h.source_id),[response.hubs[1].id,response.hubs[2].id]);
 assert.ok(hubs.filter(h=>!h.presentation).every(h=>h.items[0].kind==='album'));
 const chosen=selection.items[0];session.ensure();await flush();assert.equal(reads,1);assert.equal(session.getSnapshot().hubs[0].items[0],chosen,'selection is not recomputed by navigation/render');
 const fewer=new PlexDiscoverSession(async()=>recommendationsPage(),async()=>assert.fail());fewer.setContext(s);fewer.ensure();await flush();assert.equal(fewer.getSnapshot().hubs[0].items.length,3,'quota does not invent artists');
});
test('All omits artist-centered family and suggestions but retains source-distinct recent artists and label/genre recommendations',async()=>{
 const s=scope('a','all','en',[library('1'),library('2')]);
 const session=new PlexDiscoverSession(async(_,id)=>{const p=recommendationsPage(id);p.hubs.push({id:'recent-artists',recent_family:'played_artists',title:'Synthetic played artists',items:[{...item('501','artist',id),last_viewed_at:100}],partial:false});return p;},async()=>assert.fail());session.setContext(s);session.ensure();await flush();
 const hubs=session.getSnapshot().hubs;assert.equal(hubs.some(h=>h.artist_recommendation||h.presentation==='artist_suggestions'),false);
 assert.equal(hubs.filter(h=>!h.recent_family).length,4);assert.ok(hubs.every(h=>!h.artist_candidates));
 const recent=hubs.find(h=>h.recent_family==='played_artists');assert.equal(recent.items.length,1);assert.deepEqual(recent.items[0].artist_members.map(i=>i.library_id),['1','2']);
});
test('suggestion navigation owns composed artist references, keeps origin/return context and leaves conflicting family data unconverted',async()=>{
 const s=scope(),calls=[];let reads=0;
 const session=new PlexDiscoverSession(async()=>{reads++;return recommendationsPage();},async(server,id,kind,key)=>{calls.push({server,id,kind,key});return{id:'children',title:'Synthetic albums',items:[item('12','album',id,server)],partial:false};});session.setContext(s);session.ensure();await flush();
 const shelf=session.getSnapshot().hubs[0],artist=shelf.items[2];assert.equal(artist.rating_key,'503');assert.equal(artist.thumb.path,'/library/metadata/503/thumb/1');
 session.position=610;session.saveShelfPosition(shelf.id,240);const selected=session.artist(artist);assert.equal(selected.rating_key,'503');assert.equal(selected.library_id,'7');assert.deepEqual(calls,[]);session.open(artist);assert.equal(session.getSnapshot().detail,null);
 assert.equal(session.position,610);assert.equal(session.shelfPosition(shelf.id),240);assert.equal(session.getSnapshot().hubs[0].items[2],artist);assert.equal(reads,1);
 for(const candidate of [item('invalid','artist'),item('999','artist'),item('501','artist','foreign'),item('501','artist','7','other-server')]){
  const p=recommendationsPage();p.hubs[0].artist_candidates=[candidate];
  const invalid=new PlexDiscoverSession(async()=>p,async()=>assert.fail());invalid.setContext(s);invalid.ensure();await flush();
  assert.equal(invalid.getSnapshot().hubs.some(h=>h.presentation==='artist_suggestions'),false);assert.equal(invalid.getSnapshot().hubs[0].items[0].kind,'album');
 }
});

test('observed artist family merges two libraries chronologically while unknown and album families stay separate',async()=>{
 const s=scope('a','all','en',[library('1'),library('2')]),requests=[];
 const session=new PlexDiscoverSession((_,id,language)=>{const d=deferred();requests.push({id,d,language});return d.promise;},async()=>assert.fail());
 const response=id=>({...artistPage(id,'en',id==='2'?100:0),hubs:[...artistPage(id,'en',id==='2'?100:0).hubs,...['added','played'].map(recent_family=>({id:'album-'+recent_family,recent_family,title:'Synthetic album row',partial:false,items:[{...item('100','album',id),added_at:100,last_viewed_at:100}]}))]});
 session.setContext(s);session.ensure();requests.find(r=>r.id==='2').d.resolve(response('2'));await flush();requests.find(r=>r.id==='1').d.resolve(response('1'));await flush();
 const hubs=session.getSnapshot().hubs,artists=hubs.filter(h=>h.recent_family==='played_artists');
 assert.equal(artists.length,1);assert.equal(artists[0].items.length,11);
 assert.ok(artists[0].items.every(i=>i.kind==='artist'&&i.server_id==='a'));
 assert.deepEqual(artists[0].items.map(i=>i.library_id),Array(11).fill('2'));
 assert.ok(artists[0].items.every((i,n,list)=>!n||list[n-1].last_viewed_at>=i.last_viewed_at));
 const homonyms=artists[0].items.filter(i=>i.title==='Synthetic same-name artist');assert.equal(homonyms.length,1);assert.equal(new Set(homonyms[0].artist_members.map(discoverItemKey)).size,4);
 assert.deepEqual(hubs.filter(h=>h.recent_family).map(h=>h.recent_family),['added','played','played_artists']);
 assert.ok(hubs.filter(h=>['added','played'].includes(h.recent_family)).every(h=>h.items.length===2&&h.items.every(i=>i.kind==='album')));
 assert.equal(hubs.filter(h=>!h.recent_family).length,2,'unknown same-title groups retain separate source identities');
 assert.notEqual(hubs.filter(h=>!h.recent_family)[0].id,hubs.filter(h=>!h.recent_family)[1].id);
 const forward=mergeRecentItems(mergeRecentItems([],artistPage('1').hubs[0].items,'played_artists',s.libraries),artistPage('2').hubs[0].items,'played_artists',s.libraries);
 const reverse=mergeRecentItems(mergeRecentItems([],artistPage('2').hubs[0].items,'played_artists',s.libraries),artistPage('1').hubs[0].items,'played_artists',s.libraries);
 assert.deepEqual(forward.map(discoverItemKey),reverse.map(discoverItemKey),'equal timestamps use stable source/library order, never arrival');
 const missing={...item('9999','artist','1'),last_viewed_at:null,added_at:9999999999};
 assert.equal(mergeRecentItems([], [missing,...forward.slice(0,2)],'played_artists',s.libraries).at(-1).rating_key,'9999','addition date never substitutes for playback date');
});

test('artist aggregate retains successful sources on failure and retry navigates original artist library with stable return context',async()=>{
 const s=scope('a','all','en',[library('1'),library('2'),library('3')]),calls=[],pending=[];let fail=true;
 const session=new PlexDiscoverSession(async(_,id,language)=>{if(id==='2'&&fail)throw 'PLEX_DISCOVER_UNAVAILABLE';if(id==='3'){const d=deferred();pending.push(d);return d.promise;}return artistPage(id,language,id==='2'?100:0);},async(server,id,kind,key)=>{calls.push({server,id,kind,key});return{id:'children',title:'Synthetic albums',items:[item('888','album',id,server)],partial:false};});
 session.setContext(s);session.ensure();await flush();assert.equal(row(session,'2').phase,'error');
 const shelf=session.getSnapshot().hubs.find(h=>h.recent_family==='played_artists'),opened=shelf.items.at(-1);session.position=620;session.saveShelfPosition(shelf.id,250);const selected=session.artist(opened);await flush();
 assert.equal(selected.library_id,'1');assert.equal(selected.rating_key,opened.rating_key);assert.deepEqual(calls,[]);
 // Returning from the full artist page preserves its source card.
 session.artist(opened);pending[0].resolve(artistPage('3','en',200));await flush();
 fail=false;session.retry('2');await flush();const combined=session.getSnapshot().hubs.find(h=>h.recent_family==='played_artists');
 assert.equal(combined.items.length,11);assert.ok(combined.items.some(i=>i.artist_members.some(m=>discoverItemKey(m)===discoverItemKey(opened))),'opened artist origin remains in its presentation group');
 assert.equal(session.getSnapshot().detail,null);session.back();assert.equal(session.position,620);assert.equal(session.shelfPosition(combined.id),250);assert.ok(combined.items.some(i=>i.presentation_key===opened.presentation_key));
 assert.equal(row(session,'2').phase,'ready');assert.equal(row(session,'1').phase,'ready');
});

test('artist family rejects stale All responses after selection or account changes while individual title remains server-owned',async()=>{
 const calls=[];let active=0,max=0;
 const session=new PlexDiscoverSession((_,id,language)=>{active++;max=Math.max(max,active);const d=deferred();calls.push({id,language,d});return d.promise.finally(()=>active--);},async()=>assert.fail());
 session.setContext(scope('a','all','en',[library('1'),library('2')]));session.ensure();
 const single=scope('a','2','pt-BR',[library('1'),library('2')]);session.setContext(single);session.ensure();
 calls[0].d.resolve(artistPage('1'));calls[1].d.resolve(artistPage('2'));await flush();assert.equal(session.getSnapshot().hubs.length,0);assert.equal(row(session,'2').page,null);
 calls[2].d.resolve(artistPage('2','pt-BR'));await flush();const hub=row(session,'2').page.hubs[0];
 assert.equal(hub.items[0].library_id,'2');assert.equal(discoverHubTitle(hub,()=>assert.fail('individual title must stay unchanged')),hub.title);assert.equal(max,2);
 session.setContext({...single,identity:{}});assert.equal(row(session,'2').page,null);assert.equal(session.getSnapshot().hubs.length,0);
});

test("scoped progressive load preserves useful recommendations and does not autoplay",async()=>{
 const s=scope();let reads=0;
 const session=new PlexDiscoverSession(async()=>{reads++;return page(s);},async()=>assert.fail("not opened"));session.setContext(s);session.ensure();session.ensure();await flush();
 assert.equal(reads,1);assert.equal(row(session).phase,"ready");assert.equal(row(session).page.hubs[0].title,"Server title");
 session.position=600;session.ensure();assert.equal(reads,1);assert.equal(session.position,600);
 const album=current(session)[0];assert.deepEqual(session.album(album),{...album.album,server_id:"a",library_id:"7",library_title:"Library 7"});assert.equal(session.album({...album}),null);
});
test("rapid All/individual/credentials/language switches retain global capacity and reject obsolete replies",async()=>{
 const requests=[];let active=0,max=0;
 const session=new PlexDiscoverSession((server,id,language)=>{active++;max=Math.max(max,active);const d=deferred();requests.push({server,id,language,d});return d.promise.finally(()=>active--);},async()=>assert.fail());
 const all=scope("a","all","en",Array.from({length:12},(_,i)=>library(String(i+1)))),single=scope("a","7"),latest=scope("b","7","pt-BR");
 session.setContext(all);session.ensure();assert.equal(requests.length,2);session.setContext(single);session.ensure();session.setContext(latest);session.ensure();
 assert.equal(requests.length,2);for(let i=0;i<2;i++)requests[i].d.resolve(page({...all,libraryId:requests[i].id},[item("12","album",requests[i].id)],requests[i].id));await flush();
 assert.equal(requests.length,3);assert.equal(row(session).page,null);assert.equal(requests[2].language,"pt-BR");
 requests[2].d.resolve(page(latest));await flush();assert.equal(row(session).page.server_id,"b");assert.equal(max,2);assert.equal(requests.some(r=>r.id==="3"),false,"old queued libraries never started");
 session.setContext({...latest,identity:{}});assert.equal(row(session).page,null);assert.equal(session.position,0);
});
test("authentication and wrong-language responses remain errors; explicit retry preserves same-scope inventory",async()=>{
 const s=scope();let failure=true;
 const session=new PlexDiscoverSession(async()=>{if(failure)throw "PLEX_DISCOVER_AUTH";return page(s);},async()=>assert.fail());session.setContext(s);session.ensure();await flush();
 assert.equal(row(session).phase,"error");assert.equal(row(session).page,null);assert.equal(row(session).error,"PLEX_DISCOVER_AUTH");
 failure=false;session.retry();await flush();const previous=row(session).page;failure=true;session.retry();await flush();assert.equal(row(session).page,previous);
 const changed={...s,language:"pt-BR"};session.setContext(changed);failure=false;session.ensure();await flush();assert.equal(row(session).error,"PLEX_DISCOVER_CONTEXT");
});
test("collection details retain origin and cache bounds, ignore stale replies and preserve return positions",async()=>{
 const s=scope(),pending=[];
 const session=new PlexDiscoverSession(async()=>page(s,[item("8","collection"),item("9","collection")]),(_,id,kind,key,language)=>{const d=deferred();pending.push({id,kind,key,language,d});return d.promise;});session.setContext(s);session.ensure();await flush();session.position=400;
 const [artist,collection]=current(session);session.open(artist);session.open(collection);
 pending[0].d.resolve({id:"collection.8",title:"Collection",items:[item()],partial:false});await flush();assert.equal(session.getSnapshot().detail.item,collection);assert.equal(session.getSnapshot().detail.hub,null);
 pending[1].d.resolve({id:"collection.9",title:"Collection",items:[item()],partial:false});await flush();session.back();assert.equal(session.position,400);session.open(collection);assert.equal(pending.length,2);assert.equal(session.getSnapshot().detail.phase,"ready");assert.equal(pending[1].id,"7");assert.equal(pending[1].language,"en");
 session.setContext(scope("other","7"));session.ensure();await flush();assert.equal(row(session).error,"PLEX_DISCOVER_CONTEXT");
});
test("position and detail caches remain bounded and clear on scope changes",async()=>{
 const s=scope();let reads=0;
 const session=new PlexDiscoverSession(async()=>page(s,Array.from({length:6},(_,i)=>item(String(100+i),"collection"))),async(_,id,kind,key)=>{reads++;return{id:`${kind}.${key}`,title:"Fixture",items:[item("12","album",id)],partial:false};});session.setContext(s);session.ensure();await flush();const artists=current(session);
 for(let i=0;i<20;i++)session.saveShelfPosition(`row-${i}`,i*10);assert.equal(session.shelfPosition("row-0"),0);assert.equal(session.shelfPosition("row-19"),190);
 for(const artist of artists){session.open(artist);await flush();session.back();}
 session.open(artists[5]);await flush();assert.equal(reads,6);session.back();session.open(artists[0]);await flush();assert.equal(reads,7);
 session.setContext(s);assert.equal(session.shelfPosition("row-19"),190);session.setContext(scope());assert.equal(session.shelfPosition("row-19"),0);
});
test("explicit refresh updates rows without resetting scroll or retaining superseded objects",async()=>{
 const s=scope();let reads=0;
 const session=new PlexDiscoverSession(async()=>page(s,Array.from({length:++reads},(_,i)=>item(String(12+i)))),async()=>assert.fail());session.setContext(s);session.ensure();await flush();const old=current(session)[0];
 session.position=900;session.saveShelfPosition("server-row",300);session.retry();await flush();assert.equal(current(session).length,2);assert.equal(reads,2);assert.equal(session.position,900);assert.equal(session.shelfPosition("server-row"),300);assert.equal(session.album(old),null);
});
test("All progressively checks every library with bounded admission, global shelves and targeted partial retry",async()=>{
 const libs=Array.from({length:35},(_,i)=>library(String(i+1))),s=scope("a","all","en",libs);let fail=true,active=0,max=0;const reads=[];
 const session=new PlexDiscoverSession(async(server,id,language)=>{
  active++;max=Math.max(max,active);reads.push(id);await Promise.resolve();active--;
  if(id==="2"&&fail)throw "PLEX_DISCOVER_UNAVAILABLE";
  return {language,server_id:server,library_id:id,partial:false,hubs:[...['added','played'].map(recent_family=>({id:`opaque-${recent_family}-${id}`,recent_family,title:"Untranslated server name",partial:false,items:Array.from({length:12},(_,j)=>({...item(String(j),"album",id,server),added_at:Number(id)*1000+j,last_viewed_at:100000-Number(id)*1000+j}))})),{id:'custom',title:'Recently added',partial:false,items:[item('900','album',id,server)]}]};
 },async()=>assert.fail());
 session.setContext(s);session.ensure();assert.equal(session.queue.length,4);assert.equal(session.getSnapshot().libraries.filter(l=>l.phase==='loading').length,6);
 for(let i=0;i<30;i++)await flush();
 const snapshot=session.getSnapshot();assert.equal(snapshot.libraries.length,35);assert.equal(new Set(reads).size,35);assert.equal(max,2);assert.equal(row(session,'2').phase,'error');
 const recent=snapshot.hubs.filter(h=>h.recent_family);assert.equal(recent.length,2);assert.ok(recent.every(h=>h.items.length===DISCOVER_RECENT_LIMIT));assert.ok(snapshot.libraries.every(l=>!l.page||l.page.hubs.length===0),'per-library card arrays are not retained');
 assert.equal(recent[0].items[0].library_id,'35');assert.equal(recent[1].items[0].library_id,'1');assert.equal(snapshot.hubs.filter(h=>!h.recent_family).length,8,'custom shelves remain distinct under a global cap');
 const success=recent[0].items[0];fail=false;session.retry('2');for(let i=0;i<5;i++)await flush();assert.equal(reads.length,36);assert.equal(row(session,'2').phase,'ready');assert.ok(session.owns(success));
 const homonyms=session.getSnapshot().hubs.find(h=>h.recent_family==='played').items.filter(i=>i.rating_key==='11');assert.ok(homonyms.length>=2);assert.notEqual(discoverItemKey(homonyms[0]),discoverItemKey(homonyms[1]));
});
test("recent dates use their own fields, missing dates are separate, and completion order is irrelevant",()=>{
 const libs=[library('1'),library('2')];const a=[{...item('1','album','1'),added_at:20,last_viewed_at:100},{...item('2','album','1'),added_at:null,last_viewed_at:0}];const b=[{...item('1','album','2'),added_at:50,last_viewed_at:10},{...item('2','album','2'),added_at:NaN,last_viewed_at:5}];
 const merge=(x,y,f)=>mergeRecentItems(mergeRecentItems([],x,f,libs),y,f,libs);
 const keys=items=>items.map(discoverItemKey);
 assert.deepEqual(keys(merge(a,b,'added')),keys(merge(b,a,'added')));assert.deepEqual(merge(a,b,'added').map(i=>i.library_id),['2','1','1','2']);
 assert.deepEqual(merge(a,b,'played').map(i=>i.last_viewed_at),[100,10,5,0]);assert.equal(merge(a,b,'added').length,4,'homonymous albums are not title-deduplicated');
});
test("progressive results preserve the opened card and native shelf position within the global cap",async()=>{
 const s=scope('a','all','en',[library('1'),library('2'),library('3')]),pending=[];
 const session=new PlexDiscoverSession((server,id,language)=>{const d=deferred();pending.push({id,d,language});return d.promise;},async()=>assert.fail());session.setContext(s);session.ensure();
 const reply=id=>({language:s.language,server_id:s.serverId,library_id:id,partial:false,hubs:[{id:'opaque',recent_family:'added',title:'Server title',partial:false,items:Array.from({length:12},(_,i)=>({...item(String(i),'album',id),added_at:Number(id)*100+i}))}]});
 pending[0].d.resolve(reply('1'));await flush();const opened=session.getSnapshot().hubs[0].items.at(-1);assert.equal(session.album(opened).library_id,'1');session.position=530;session.saveShelfPosition(session.getSnapshot().hubs[0].id,210);
 pending[1].d.resolve(reply('2'));await flush();pending.find(r=>r.id==='3').d.resolve(reply('3'));await flush();
 const shelf=session.getSnapshot().hubs[0];assert.equal(shelf.items.length,24);assert.ok(shelf.items.includes(opened));assert.equal(session.position,530);assert.equal(session.shelfPosition(shelf.id),210);assert.equal(session.album(opened).library_id,'1');
});
test("All artist/collection/album navigation passes original library and rejects cross-library payloads",async()=>{
 const s=scope("a","all","en",[library("1"),library("2")]),calls=[];
 const session=new PlexDiscoverSession(async(_,id)=>page(s,[item("88","artist",id),item("89","collection",id),item("12","album",id)],id),async(server,id,kind,key)=>{calls.push({server,id,kind,key});return{id:"children",title:"Selection",items:[item("12","album",id)],partial:false};});
 session.setContext(s);session.ensure();await flush();const artists=session.getSnapshot().hubs.find(h=>h.library_id==="2").items;const artist=session.artist(artists[0]);assert.equal(artist.server_id,"a");assert.equal(artist.library_id,"2");assert.equal(artist.rating_key,"88");session.open(artists[0]);assert.equal(session.getSnapshot().detail,null);session.open(artists[1]);await flush();assert.deepEqual(calls[0],{server:"a",id:"2",kind:"collection",key:"89"});
 const detailAlbum=session.getSnapshot().detail.hub.items[0];assert.equal(session.album(detailAlbum).library_id,"2");session.back();session.open(artists[1]);await flush();assert.equal(calls.length,1,"collection cache preserved");
 const bad=new PlexDiscoverSession(async(_,id)=>page(s,[item("12","album","foreign")],id),async()=>assert.fail());bad.setContext(s);bad.ensure();await flush();assert.ok(bad.getSnapshot().libraries.every(l=>l.error==="PLEX_DISCOVER_CONTEXT"));
});
test("language mapping and server title fallback do not modify unknown/custom/proper names",()=>{
 assert.equal(discoverLanguage("pt-PT"),"pt-BR");assert.equal(discoverLanguage("en-US"),"en");
 const builtin={id:"scoped",source_id:"home.music.recent",context:"hub.home.music.recent",title:"Recently Added Music"};const t=key=>key==="discover.standard.recentMusic"?"Músicas adicionadas recentemente":assert.fail(key);
 assert.equal(discoverHubTitle(builtin,t),"Músicas adicionadas recentemente");
 for(const title of ["More from JVC","Minha coleção 🎵","Recently Added Music — custom","Músicas adicionadas recentemente"]){assert.equal(discoverHubTitle({...builtin,title},t),title);}
 assert.equal(discoverHubTitle({...builtin,source_id:"custom.collection"},t),builtin.title);assert.equal(discoverHubTitle({...builtin,context:"custom"},t),builtin.title);
});
test("explicit detail navigation precedes queued background work without exceeding admission",async()=>{
 const s=scope("a","all","en",Array.from({length:8},(_,i)=>library(String(i+1)))),loads=[],detail=deferred();let opened=0;
 const session=new PlexDiscoverSession((server,id,language)=>{const d=deferred();loads.push({server,id,language,d});return d.promise;},()=>{opened++;return detail.promise;});session.setContext(s);session.ensure();
 const reply=id=>page(s,[item("88","collection",id)],id);
 loads[0].d.resolve(reply("1"));await flush();assert.equal(loads.length,3);
 session.open(session.getSnapshot().hubs.find(h=>h.library_id==="1").items[0]);
 loads[1].d.resolve(reply("2"));await flush();assert.equal(opened,1);assert.equal(loads.length,3,"queued hub jobs cannot block explicit detail");assert.equal(session.running,2);
 detail.resolve({id:"collection.88",title:"Collection",partial:false,items:[item("12","album","1")]});await flush();assert.equal(session.getSnapshot().detail.phase,"ready");
 session.setContext(null);for(const request of loads.slice(2))request.d.resolve(reply(request.id));await flush();assert.equal(session.running,0);assert.equal(session.queue.length,0);
});

test('recent artists conservatively group names, retain real origins and rank newest with stable ties before the cap',()=>{
 const libs=[library('1'),library('2')];
 const artists=[['1','1','  João  Gilberto ',100],['2','2','JOA\u0303O GILBERTO',200],['3','2','Joao Gilberto',300],['4','2','João-Gilberto',400],['5','1','Unknown artist',500],['6','2','Unknown artist',600],['7','1','',700],['8','2','',800]].map(([key,id,title,last_viewed_at])=>({...item(key,'artist',id),title,last_viewed_at}));
 const grouped=mergeRecentItems([],artists,'played_artists',libs);assert.equal(grouped.length,7);
 const joao=grouped.find(i=>i.rating_key==='2');assert.equal(joao.artist_members.length,2);assert.equal(joao.library_id,'2');assert.equal(joao.server_id,'a');
 assert.equal(new Set(grouped.map(i=>i.presentation_key)).size,7);
 const tied=artists.slice(0,2).map(i=>({...i,last_viewed_at:200}));assert.deepEqual(mergeRecentItems(mergeRecentItems([],tied.slice(0,1),'played_artists',libs),tied.slice(1),'played_artists',libs),mergeRecentItems(mergeRecentItems([],tied.slice(1),'played_artists',libs),tied.slice(0,1),'played_artists',libs),'stable ties must not use library response arrival');
 assert.equal(mergeRecentItems([],artists,'played',libs).length,8,'other families keep original identity');
 const many=Array.from({length:30},(_,i)=>({...item(String(i+20),'artist','1'),title:`Artist ${i}`,last_viewed_at:1000-i}));assert.equal(mergeRecentItems([],many,'played_artists',libs).length,24);
});
