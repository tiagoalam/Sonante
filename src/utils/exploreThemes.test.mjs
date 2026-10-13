import test from "node:test";
import assert from "node:assert/strict";
import { eligibleExploreThemes, exploreThemeYear } from "./exploreThemes.ts";
import { createExploreSession } from "./explore.ts";
import { groupExplorePresentation } from "./explorePresentation.ts";

const local = (id, year = "1995", genre = "Jazz") => ({ id, title: `Album ${id}`, artist: `Artist ${id}`, year, genres: [genre], folder_path: `Synthetic/${id}`, discs: [], track_count: 1 });
const plex = album => ({ rating_key: album.id, title: album.title, artist: album.artist, year: Number(String(album.year).slice(0,4)), genres: album.genres, artist_rating_key: album.artist });
const context = (albums, source = "both") => ({ local: { albums: source === "plex" ? [] : albums, state: source === "plex" ? "not_configured" : "ready" },
  plex: { albums: source === "local" ? [] : albums.map(plex), state: source === "local" ? "not_configured" : "ready", identity: {}, serverId: "server", libraryId: "one" } });
const entries = albums => albums.map(album => ({ key: album.id, source: "local", album, version: albums }));
const now = () => new Date("2026-07-01T12:00:00Z");
const keys = items => items.map(entry => entry.key);

test("theme years require explicit real nonfuture metadata; the current decade follows the injected date", () => {
  for (const value of [undefined, null, "", "unknown", "199x", "1995 remaster", 1995.5, 999, 2027, "2026-02-30", "2026-13", "2026-00-01"]) assert.equal(exploreThemeYear(value, 2026), null);
  assert.equal(exploreThemeYear("1995-06-13", 2026),1995);assert.equal(exploreThemeYear(2026,2026),2026);
  const albums=Array.from({length:10},(_,i)=>local(String(i),"2035"));
  assert.equal(eligibleExploreThemes(entries(albums),2026).filter(t=>t.id==="decade").length,0);
  assert.deepEqual(eligibleExploreThemes(entries(albums),2036).find(t=>t.id==="decade"),{id:"decade",value:2030,current:true});
  assert.equal(eligibleExploreThemes(entries(albums),2041).find(t=>t.id==="decade").current,false);
});

test("eligibility counts ten presentation groups, never Local/Plex copies, and excludes only genre placeholders",()=>{
  const nine=Array.from({length:9},(_,i)=>local(String(i)));
  const duplicates=entries([...nine,...nine.map(a=>({...a,id:`copy-${a.id}`}))]);
  assert.deepEqual(eligibleExploreThemes(duplicates,2026),[]);
  const ten=[...nine,local("tenth")];assert.equal(eligibleExploreThemes(entries(ten),2026).length,2);
  for(const placeholder of ["Unknown"," DESCONHECIDO ","unknown genre","N/A","sem gênero",""]){
    const albums=ten.map(a=>({...a,genres:[placeholder]}));assert.equal(eligibleExploreThemes(entries(albums),2026).filter(t=>t.id==="genre").length,0);
  }
  const punctuation=ten.map(a=>({...a,genres:[" Jazz; Rock "]}));
  assert.deepEqual(eligibleExploreThemes(entries(punctuation),2026).filter(t=>t.id==="genre"),[{id:"genre",value:"jazz",label:"Jazz"},{id:"genre",value:"rock",label:"Rock"}]);
});

test("both thematic shelves work for isolated sources, retain original items and prefer eligible Local copies",()=>{
  const albums=Array.from({length:14},(_,i)=>local(String(i)));
  for(const source of ["local","plex","both"]){
    const c=context(albums,source),s=createExploreSession(()=>0.3,now);s.syncContext(c,true);
    assert.equal(s.getSnapshot().themes.length,2);
    for(const theme of s.getSnapshot().themes){
      assert.equal(theme.selected.length,10);assert.equal(groupExplorePresentation(theme.selected).length,10);
      assert.ok(theme.selected.every(e=>c[e.source].albums.includes(e.album)));
      assert.ok(theme.selected.every(e=>e.source===(source==="plex"?"plex":"local")));
      assert.ok(theme.selected.every(e=>s.canUseShelf(e,s.getSnapshot().revision,theme.id)));
    }
  }
});

test("individual renewal changes only its shelf/reset, preserves theme and order, and uses ten new groups when available",()=>{
  const albums=Array.from({length:20},(_,i)=>local(String(i))),s=createExploreSession(()=>0,now);s.syncContext(context(albums,"local"),true);
  const before=s.getSnapshot(),old=before.themes.find(t=>t.id==="decade");s.renewShelf("decade");const after=s.getSnapshot(),fresh=after.themes.find(t=>t.id==="decade");
  assert.equal(fresh.value,old.value);assert.deepEqual(after.sectionOrder,before.sectionOrder);assert.equal(after.selected,before.selected);assert.equal(after.artists,before.artists);assert.equal(after.genres,before.genres);
  assert.equal(after.themes.find(t=>t.id==="genre"),before.themes.find(t=>t.id==="genre"));assert.deepEqual(after.resets,{decade:1});
  assert.ok(fresh.selected.every(entry=>!old.selected.some(e=>e.key===entry.key)));
  s.renewShelf("albums");assert.deepEqual(s.getSnapshot().resets,{decade:1,albums:1});assert.equal(s.getSnapshot().themes,after.themes);
  s.renewShelf("artists");assert.equal(s.getSnapshot().resets.artists,1);assert.equal(s.getSnapshot().themes,after.themes);
  const exact=createExploreSession(()=>0,now);exact.syncContext(context(albums.slice(0,10),"local"),true);exact.renewShelf("genre");assert.equal(exact.getSnapshot().themes.find(t=>t.id==="genre").selected.length,10);
});

test("general renewal may change themes/order; rerenders, detail return and catalog additions cannot do so",()=>{
  let draw=0;const s=createExploreSession(()=>draw,now);
  const albums=Array.from({length:40},(_,i)=>local(String(i),i<20?"1995":"2005",i<20?"Jazz":"Rock")),c=context(albums,"local");s.syncContext(c,true);
  const first=s.getSnapshot();assert.equal(first.themes.find(t=>t.id==="decade").value,1990);
  for(let i=0;i<8;i++)s.syncContext(c,true);assert.equal(s.getSnapshot(),first);
  s.syncContext(c,false);s.syncContext(c,true);assert.deepEqual(s.getSnapshot().themes,first.themes);assert.deepEqual(s.getSnapshot().sectionOrder,first.sectionOrder);
  const next={...c,plex:{...c.plex,state:"ready",albums:Array.from({length:12},(_,i)=>plex(local(`late-${i}`,"2025","Soul")))}};
  s.syncContext(next,true);assert.deepEqual(s.getSnapshot().themes.map(t=>keys(t.selected)),first.themes.map(t=>keys(t.selected)));assert.deepEqual(s.getSnapshot().sectionOrder,first.sectionOrder);
  draw=0.99;s.renew();const renewed=s.getSnapshot();assert.equal(renewed.themes.find(t=>t.id==="decade").value,2020);assert.equal(renewed.themes.find(t=>t.id==="decade").current,true);
  assert.notDeepEqual(renewed.sectionOrder,first.sectionOrder);assert.ok(Object.values(renewed.resets).every(n=>n===1));
  const old=renewed.themes.find(t=>t.id==="decade").selected[0];s.syncContext({...next,plex:{...next.plex,identity:{},state:"unavailable"}},true);
  assert.equal(s.canUseShelf(old,renewed.revision,"decade"),false);assert.equal(s.getSnapshot().themes.some(t=>t.id==="decade"),false);
});

test("artist album fallback follows group identity and stays with one chosen item despite cache/catalog arrivals",()=>{
  const albums=[local("a"),local("b")];albums[1].artist=albums[0].artist;
  const c=context(albums),s=createExploreSession(()=>0,now);s.syncContext(c,true);
  const localGroup=s.getSnapshot().artists.find(g=>g.source==="local"),plexGroup=s.getSnapshot().artists.find(g=>g.source==="plex");
  const cached=entry=>entry.source==="local"&&entry.album.id==="b";
  const chosen=s.artistAlbum(localGroup,cached);assert.equal(chosen.album.id,"b");assert.equal(chosen.source,"local");
  assert.equal(s.artistAlbum(localGroup,()=>true),chosen);
  const other=s.artistAlbum(plexGroup);assert.equal(other.source,"plex");assert.ok(other.album.artist_rating_key);
  s.syncContext({...c,plex:{...c.plex,albums:[...c.plex.albums,plex(local("new"))]}},true);
  assert.equal(s.artistAlbum(localGroup).album,chosen.album);
  s.syncContext({...c,local:{...c.local,state:"unavailable"}},true);assert.equal(s.artistAlbum(localGroup),undefined);
});
