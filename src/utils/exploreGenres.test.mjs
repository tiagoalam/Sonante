import test from "node:test";
import assert from "node:assert/strict";
import { exploreGenreNames, deriveExploreGenres, filterExploreAlbumsByGenre } from "./exploreGenres.ts";
import { createExploreSession } from "./explore.ts";

const album = (id, genres) => ({ id, title: `Album ${id}`, artist: `Artist ${id}`, genres, folder_path: "Synthetic", discs: [], track_count: 1 });
const context = (local, plex = []) => ({ local: { albums: local, state: local.length ? "ready" : "not_configured" },
  plex: { albums: plex, state: plex.length ? "ready" : "not_configured", identity: {}, serverId: "fixture", libraryId: "one" } });
const copy = a => ({ rating_key: `P-${a.id}`, title: a.title, artist: a.artist, genres: a.genres });

test("the explicit Rock discovery family unions list associations once per source item without modifying other styles",()=>{
  const variants=[" ROCK ","Rock & Roll","rock   and roll"];
  const local=Array.from({length:10},(_,i)=>album(`rock-${i}`,[variants[i%3],"Rock; Rock & Roll | Rock and Roll"]));
  const plex=local.map(copy),before=JSON.stringify([local,plex]);
  const groups=deriveExploreGenres([...local,...plex]);
  assert.deepEqual(groups,[{value:"rock",label:"Rock",albumCount:20}]);
  for(const variant of variants)assert.deepEqual(filterExploreAlbumsByGenre([...local,...plex],variant),[...local,...plex]);
  assert.deepEqual(exploreGenreNames({genres:["Rock & Roll, Jazz", "Rock and Roll/Rock", "Hard Bop", "World", "Rock-and-Roll", "Rock 'n' Roll"]}),["Rock","Jazz","Hard Bop","World","Rock-and-Roll","Rock 'n' Roll"]);
  for(const mode of ["local","plex","both"]){
    const s=createExploreSession(()=>0.2);s.syncContext(context(mode==="plex"?[]:local,mode==="local"?[]:plex),true);
    const group=s.getSnapshot().genres[0];assert.equal(group.value,"rock");assert.equal(group.name,"Rock");assert.equal(s.getSnapshot().genres.length,1);
    assert.equal(s.getSnapshot().themes.find(t=>t.id==="genre").value,"rock");assert.equal(s.getSnapshot().themes.find(t=>t.id==="genre").selected.length,10);
    s.openSlice(group,s.getSnapshot().revision);const entries=s.getSnapshot().slice.entries;
    assert.equal(entries.length,mode==="both"?20:10);assert.equal(new Set(entries.map(e=>e.key)).size,entries.length);
    assert.equal(s.genreAlbumCount(group),entries.length);
  }
  assert.equal(JSON.stringify([local,plex]),before);
});

test("Rock family copies do not inflate five-card or ten-theme thresholds; other subgenres stay separate",()=>{
  const local=Array.from({length:10},(_,i)=>album(`threshold-${i}`,[i%2?"Rock & Roll":"Rock and Roll"]));
  for(const count of [4,5,9,10]){
    const albums=local.slice(0,count),s=createExploreSession(()=>0.2);s.syncContext(context(albums,albums.map(copy)),true);
    assert.equal(s.getSnapshot().genres.length,count>=5?1:0);
    assert.equal(s.getSnapshot().themes.length,count>=10?1:0);
  }
  const separated=deriveExploreGenres([album("jazz",["Jazz"]),album("bop",["Hard Bop"]),album("world",["World"])]);
  assert.deepEqual(separated.map(g=>[g.value,g.albumCount]),[["hard bop",1],["jazz",1],["world",1]]);
});

test("simple lists and a bounded recognized slash chain produce exact names without splitting protected phrases",()=>{
  assert.deepEqual(exploreGenreNames({genres:["Chicago Blues, Unplugged"]}),["Chicago Blues","Unplugged"]);
  assert.deepEqual(exploreGenreNames({genres:["Big Beat/Hip Hop/Breakbeat Hardcore/Acid Breaks/Industrial"]}),["Big Beat","Hip Hop","Breakbeat Hardcore","Acid Breaks","Industrial"]);
  assert.deepEqual(exploreGenreNames({genres:["Hip Hop; R&B | Rock and Roll, Rock-and-Roll"]}),["Hip Hop","R&B","Rock","Rock-and-Roll"]);
  assert.deepEqual(exploreGenreNames({genres:["Pop / Rock", "R&B / Soul"]}),["Pop/Rock","R&B/Soul"]);
  assert.deepEqual(exploreGenreNames({genres:["Rock \'n\' Roll, Hip Hop"]}),["Rock \'n\' Roll","Hip Hop"]);
});

test("ambiguous text remains in the input but cannot become discovery names; separate valid tags still work",()=>{
  const ambiguous=["Experimental/My Own Style","Experimental, My Own Style","Jazz/Fictional","Jazz/","Jazz,,Blues",'"Jazz, Blues"',"Rock (Alternative, Indie)", "\'Jazz, Blues\'",
    "Big Beat/Hip Hop/Industrial…","Chicago Blues + Live","Jazz: Fusion","Jazz\\Blues","Long".repeat(30),Array(17).fill("Jazz").join(";")];
  for(const raw of ambiguous){const a={genres:[raw,"Soul"]},before=JSON.stringify(a);assert.deepEqual(exploreGenreNames(a),["Soul"],raw);assert.equal(JSON.stringify(a),before);}
  assert.deepEqual(exploreGenreNames({genres:["Unknown, Jazz","Desconhecido", "N/A"]}),["Jazz"]);
});

test("NFC/case/space grouping connects Local/Plex list members to the same discovery slice without changing their tags",()=>{
  const a=album("L",[" Chicago   Blues , Unplugged", "MU\u0301SICA  BRASILEIRA"]),b=copy(album("P",["chicago blues","Música Brasileira"]));
  const before=JSON.stringify([a,b]),groups=deriveExploreGenres([a,b]);
  assert.equal(groups.find(g=>g.value==="chicago blues").albumCount,2);
  assert.equal(groups.find(g=>g.value==="música brasileira").albumCount,2);
  assert.deepEqual(filterExploreAlbumsByGenre([a,b],"chicago blues"),[a,b]);assert.equal(JSON.stringify([a,b]),before);
  const s=createExploreSession(()=>0.2);const extra=Array.from({length:3},(_,i)=>album(`extra-${i}`,["Chicago Blues"]));s.syncContext(context([a,...extra],[b]),true);const group=s.getSnapshot().genres.find(g=>g.value==="chicago blues");assert.ok(group);
  s.openSlice(group,s.getSnapshot().revision);assert.deepEqual(s.getSnapshot().slice.entries.map(e=>e.album),[a,...extra,b]);assert.equal(s.genreAlbumCount(group),5);
});

test("navigation requires five presentation groups; thematic albums require ten; displayed counts retain actual source entries",()=>{
  const a=album("A",["Chicago Blues, Unplugged"]),b=album("B",["Chicago Blues"]),s=createExploreSession(()=>0.2);
  s.syncContext(context([a],[copy(a)]),true);assert.equal(s.getSnapshot().genres.length,0,"two source copies are one presentation group");
  const c=context([a,b,...Array.from({length:3},(_,i)=>album(`extra-${i}`,["Chicago Blues"]))],[copy(a)]);s.syncContext(c,true);s.renewShelf("genres");const group=s.getSnapshot().genres[0];
  assert.equal(group.value,"chicago blues");assert.equal(s.getSnapshot().genres.length,1,"Unplugged has only one group");assert.equal(s.genreAlbumCount(group),6);
  s.openSlice(group,s.getSnapshot().revision);assert.equal(s.getSnapshot().slice.entries.length,6);assert.equal(s.getSnapshot().themes.length,0);
  const ten=Array.from({length:10},(_,i)=>album(String(i),["Chicago Blues, Unplugged"]));
  for(const mode of ["local","plex","both"]){const current=context(mode==="plex"?[]:ten,mode==="local"?[]:ten.map(copy));const session=createExploreSession(()=>0.2);session.syncContext(current,true);
    assert.equal(session.getSnapshot().genres.length,2);assert.equal(session.getSnapshot().themes.find(t=>t.id==="genre").selected.length,10);}
});
