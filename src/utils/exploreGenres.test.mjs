import test from "node:test";
import assert from "node:assert/strict";
import { exploreGenreNames, deriveExploreGenres, filterExploreAlbumsByGenre } from "./exploreGenres.ts";
import { createExploreSession } from "./explore.ts";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";

const album = (id, genres) => ({ id, title: `Album ${id}`, artist: `Artist ${id}`, genres, source_id: "disk", folder_path: "Synthetic", discs: [], track_count: 1 });
const context = (albums, sourceId = null) => {
  const local = filterLocalAlbumsByScope(albums, sourceId, "");
  return { local: { albums: local, state: local.length ? "ready" : "not_configured", sourceId } };
};
const copy = a => ({ rating_key: `P-${a.id}`, title: a.title, artist: a.artist, genres: a.genres });
const localCopy = a => ({ ...a, id: `NAS-${a.id}`, source_id: "nas", folder_path: `Synthetic/NAS-${a.id}` });

test("the explicit Rock discovery family unions list associations once per source item without modifying other styles",()=>{
  const variants=[" ROCK ","Rock & Roll","rock   and roll"];
  const local=Array.from({length:10},(_,i)=>album(`rock-${i}`,[variants[i%3],"Rock; Rock & Roll | Rock and Roll"]));
  const plex=local.map(copy),before=JSON.stringify([local,plex]);
  const groups=deriveExploreGenres([...local,...plex]);
  assert.deepEqual(groups,[{value:"rock",label:"Rock",albumCount:20}]);
  for(const variant of variants)assert.deepEqual(filterExploreAlbumsByGenre([...local,...plex],variant),[...local,...plex]);
  assert.deepEqual(exploreGenreNames({genres:["Rock & Roll, Jazz", "Rock and Roll/Rock", "Hard Bop", "World", "Rock-and-Roll", "Rock 'n' Roll"]}),["Rock","Jazz","Hard Bop","World","Rock-and-Roll","Rock 'n' Roll"]);
  const copies=local.map(localCopy),inventory=[...local,...copies],localBefore=JSON.stringify(inventory);
  for(const sourceId of ["disk","nas",null]){
    const s=createExploreSession(()=>0.2);s.syncContext(context(inventory,sourceId),true);
    const group=s.getSnapshot().genres[0];assert.equal(group.value,"rock");assert.equal(group.name,"Rock");assert.equal(s.getSnapshot().genres.length,1);
    assert.equal(s.getSnapshot().themes.find(t=>t.id==="genre").value,"rock");assert.equal(s.getSnapshot().themes.find(t=>t.id==="genre").selected.length,10);
    s.openSlice(group,s.getSnapshot().revision);const entries=s.getSnapshot().slice.entries;
    assert.equal(entries.length,sourceId===null?20:10);assert.equal(new Set(entries.map(e=>e.key)).size,entries.length);
    assert.deepEqual(entries.map(e=>e.album),sourceId==="disk"?local:sourceId==="nas"?copies:inventory);
    assert.ok(entries.every(e=>e.source==="local"));
    assert.equal(s.genreAlbumCount(group),entries.length);
  }
  assert.equal(JSON.stringify(inventory),localBefore);
  assert.equal(JSON.stringify([local,plex]),before);
});

test("Rock family copies do not inflate five-card or ten-theme thresholds; other subgenres stay separate",()=>{
  const local=Array.from({length:10},(_,i)=>album(`threshold-${i}`,[i%2?"Rock & Roll":"Rock and Roll"]));
  for(const count of [4,5,9,10]){
    const originals=local.slice(0,count),albums=[...originals,...originals.map(localCopy)],s=createExploreSession(()=>0.2);s.syncContext(context(albums),true);
    assert.equal(s.getSnapshot().genres.length,count>=5?1:0);
    assert.equal(s.getSnapshot().themes.length,count>=10?1:0);
    assert.equal(s.getSnapshot().selected.length,count,"Local duplicates must not inflate presentation suggestions");
    if(count>=5){const group=s.getSnapshot().genres[0];s.openSlice(group,s.getSnapshot().revision);assert.equal(s.genreAlbumCount(group),count*2);assert.equal(s.getSnapshot().slice.entries.length,count*2);assert.equal(new Set(s.getSnapshot().slice.entries.map(e=>e.album.id)).size,count*2);}
    if(count===10){const theme=s.getSnapshot().themes[0];assert.equal(theme.selected.length,10);assert.equal(new Set(theme.selected.map(e=>e.album.title)).size,10);}
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

test("NFC/case/space grouping preserves pure Local/Plex parsing and connects Local list members without changing tags",()=>{
  const a=album("L",[" Chicago   Blues , Unplugged", "MU\u0301SICA  BRASILEIRA"]),b=copy(album("P",["chicago blues","Música Brasileira"]));
  const before=JSON.stringify([a,b]),groups=deriveExploreGenres([a,b]);
  assert.equal(groups.find(g=>g.value==="chicago blues").albumCount,2);
  assert.equal(groups.find(g=>g.value==="música brasileira").albumCount,2);
  assert.deepEqual(filterExploreAlbumsByGenre([a,b],"chicago blues"),[a,b]);assert.equal(JSON.stringify([a,b]),before);
  const localB={...album("second",b.genres),source_id:"nas"};
  const s=createExploreSession(()=>0.2),extra=Array.from({length:3},(_,i)=>album(`extra-${i}`,["Chicago Blues"])),inventory=[a,...extra,localB],localBefore=JSON.stringify(inventory);
  s.syncContext(context(inventory),true);const group=s.getSnapshot().genres.find(g=>g.value==="chicago blues");assert.ok(group);
  s.openSlice(group,s.getSnapshot().revision);assert.deepEqual(s.getSnapshot().slice.entries.map(e=>e.album),inventory);assert.equal(s.genreAlbumCount(group),5);
  assert.equal(JSON.stringify(inventory),localBefore);assert.equal(JSON.stringify([a,b]),before);
});

test("navigation requires five presentation groups; thematic albums require ten; displayed counts retain actual source entries",()=>{
  const a=album("A",["Chicago Blues, Unplugged"]),b=album("B",["Chicago Blues"]),s=createExploreSession(()=>0.2);
  s.syncContext(context([a,localCopy(a)]),true);assert.equal(s.getSnapshot().genres.length,0,"two Local source copies are one presentation group");
  const c=context([a,b,...Array.from({length:3},(_,i)=>album(`extra-${i}`,["Chicago Blues"])),localCopy(a)]);s.syncContext(c,true);s.renewShelf("genres");const group=s.getSnapshot().genres[0];
  assert.equal(group.value,"chicago blues");assert.equal(s.getSnapshot().genres.length,1,"Unplugged has only one group");assert.equal(s.genreAlbumCount(group),6);
  s.openSlice(group,s.getSnapshot().revision);assert.equal(s.getSnapshot().slice.entries.length,6);assert.equal(s.getSnapshot().themes.length,0);
  const ten=Array.from({length:10},(_,i)=>album(String(i),["Chicago Blues, Unplugged"]));
  const inventory=[...ten,...ten.map(localCopy)];
  for(const sourceId of ["disk","nas",null]){const current=context(inventory,sourceId),session=createExploreSession(()=>0.2);session.syncContext(current,true);
    assert.equal(session.getSnapshot().genres.length,2);assert.equal(session.getSnapshot().themes.find(t=>t.id==="genre").selected.length,10);
    for(const genre of session.getSnapshot().genres){session.openSlice(genre,session.getSnapshot().revision);assert.equal(session.genreAlbumCount(genre),sourceId===null?20:10);assert.equal(session.getSnapshot().slice.entries.length,session.genreAlbumCount(genre));}
  }
});
