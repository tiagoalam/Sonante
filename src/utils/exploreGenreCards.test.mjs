import test from "node:test";
import assert from "node:assert/strict";
import { EXPLORE_GENRE_PALETTE, exploreGenreCardStyle, assignExploreGenreColors, assignExploreGenreIcons } from "./exploreGenreCards.ts";
import { createExploreSession } from "./explore.ts";
import { isExploreGenreName } from "./exploreThemes.ts";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";

test("genre identity alone determines a finite repeated palette after surface normalization",()=>{
  assert.deepEqual(exploreGenreCardStyle("  JAZZ  "),exploreGenreCardStyle("jazz"));
  assert.deepEqual(exploreGenreCardStyle("MU\u0301SICA  BRASILEIRA"),exploreGenreCardStyle("música brasileira"));
  const names=["Jazz","Rock","Blues","Pop","Samba","Reggae","Country","Soul","Funk","Ambient","Classical","Electronic"];
  const first=names.map(exploreGenreCardStyle);
  assert.deepEqual([...names].reverse().map(exploreGenreCardStyle).reverse(),first);
  assert.ok(first.every(style=>EXPLORE_GENRE_PALETTE.includes(style.background)));
  assert.ok(new Set(first.map(style=>style.background)).size<names.length);
});

test("known genre icons are decorative exact-label hints; unknown/composite tags retain their identity",()=>{
  assert.equal(exploreGenreCardStyle("Rock").icon,"guitar");assert.equal(exploreGenreCardStyle("Clássica").icon,"piano");
  assert.equal(exploreGenreCardStyle(" Hip   Hop ").icon,"mic");assert.equal(exploreGenreCardStyle("Electronic").icon,"headphones");
  for(const name of ["Jazz; Rock","Rock/Jazz","Experimental folk", "My own tag"])assert.equal(exploreGenreCardStyle(name).icon,"music");
});

test("displayed icons preserve preferences and resolve collisions deterministically using neutral musical alternatives",()=>{
  for(const variant of ["Rock"," ROCK & ROLL ","rock   and roll"])assert.equal(exploreGenreCardStyle(variant).icon,"guitar");
  assert.equal(exploreGenreCardStyle("Hard Bop").icon,"piano");
  const values=["Rock","Hard Bop","World","Jazz"],icons=assignExploreGenreIcons(values);
  assert.equal(new Set(Object.values(icons)).size,4);assert.equal(icons.rock,"guitar");assert.equal(icons["hard bop"],"piano");
  assert.equal(icons.world,"music");assert.equal(icons.jazz,"disc");
  assert.deepEqual(assignExploreGenreIcons([...values].reverse()),icons);
  assert.deepEqual(assignExploreGenreIcons(values.map(v=>` ${v.toUpperCase()} `)),icons);
  for(const names of [["A","B","C","D"],["Rock","Blues","Metal","Guitar music"],["constructor","__proto__","A","B"]]){
    const assigned=assignExploreGenreIcons(names);assert.equal(new Set(Object.values(assigned)).size,4);
    assert.deepEqual(assignExploreGenreIcons([...names].reverse()),assigned);
    assert.ok(Object.values(assigned).every(icon=>["guitar","music","disc","audio","playlist"].includes(icon)));
  }
});

test("palette maintains readable title/secondary contrast including highlight gradient and hover brightness",()=>{
  const rgb=hex=>hex.slice(1).match(/../g).map(channel=>parseInt(channel,16)/255);
  const luminance=channels=>channels.map(v=>v<=0.04045?v/12.92:((v+0.055)/1.055)**2.4).reduce((sum,v,i)=>sum+v*[0.2126,0.7152,0.0722][i],0);
  for(const background of EXPLORE_GENRE_PALETTE)for(const foreground of ["#FFFFFF","#F3EDE7"])for(const brightness of [1,1.1]){
    const back=rgb(background).map(v=>Math.min(1,(v*0.94+0.06)*brightness));
    const front=rgb(foreground).map(v=>Math.min(1,v*brightness));
    assert.ok((luminance(front)+0.05)/(luminance(back)+0.05)>=4.5,background);
  }
});

test("navigation excludes shared placeholders with five groups while keeping the thematic ten-group minimum and counts the actual eligible slice",()=>{
  for(const name of ["Unknown"," DESCONHECIDO ","unknown genre","N/A","", "sem gênero"])assert.equal(isExploreGenreName(name),false);
  const local={id:"local",title:"Album",artist:"Artist",genres:["Jazz"," JAZZ ","Unknown"],source_id:"disk",folder_path:"Synthetic",discs:[],track_count:1};
  const duplicate={...local,id:"nas-copy",source_id:"nas",genres:["jazz","Desconhecido"]};
  const albums=[local,duplicate,...Array.from({length:4},(_,i)=>({...local,id:`other-${i}`,title:`Other album ${i}`,artist:`Other artist ${i}`}))],before=JSON.stringify(albums);
  const c={local:{albums,state:"ready",sourceId:null}};
  const s=createExploreSession(()=>0.4);s.syncContext(c,true);let group=s.getSnapshot().genres[0];
  assert.equal(s.getSnapshot().genres.length,1);assert.equal(group.value,"jazz");assert.equal(s.getSnapshot().themes.length,0);
  assert.equal(s.genreAlbumCount(group),6);s.openSlice(group,s.getSnapshot().revision);assert.equal(s.getSnapshot().slice.entries.length,s.genreAlbumCount(group));
  assert.deepEqual(s.getSnapshot().slice.entries.map(e=>e.album),albums);assert.equal(new Set(s.getSnapshot().slice.entries.map(e=>e.key)).size,6);
  assert.equal(s.getSnapshot().selected.length,5,"six Local entries represent five distinct groups");
  const style=exploreGenreCardStyle(group.value),groupKey=group.key,selected=s.getSnapshot().selected;
  s.leaveSlice(s.getSnapshot().revision);s.renewShelf("genres");assert.equal(s.getSnapshot().selected,selected);assert.equal(s.getSnapshot().genres[0].key,groupKey);assert.deepEqual(exploreGenreCardStyle(s.getSnapshot().genres[0].value),style);
  s.syncContext(c,false);s.syncContext(c,true);assert.equal(s.getSnapshot().selected,selected);assert.equal(s.getSnapshot().genres[0].key,groupKey);
  group=s.getSnapshot().genres[0];s.openSlice(group,s.getSnapshot().revision);const oldEntry=s.getSnapshot().slice.entries[0],oldRevision=s.getSnapshot().revision;
  s.syncContext({...c,local:{...c.local,state:"unavailable"}},true);
  assert.equal(s.genreAlbumCount(group),0);assert.deepEqual(s.visibleGenres(),[]);assert.deepEqual(s.visibleEntries(),[]);assert.deepEqual(s.getSnapshot().themes,[]);assert.equal(s.getSnapshot().slice,null);
  assert.equal(s.validDetail(oldEntry),false);assert.equal(s.canUseSlice(oldEntry,oldRevision),false);
  const unavailable=s.getSnapshot();s.openSlice(group,unavailable.revision);assert.equal(s.getSnapshot(),unavailable,"unavailable Local must not expose another source or an old slice");
  s.syncContext({local:{albums:filterLocalAlbumsByScope(albums,"disk",""),state:"ready",sourceId:"disk"}},true);group=s.getSnapshot().genres[0];
  assert.equal(group.key,groupKey);assert.deepEqual(exploreGenreCardStyle(group.value),style);assert.equal(s.genreAlbumCount(group),5);
  s.openSlice(group,s.getSnapshot().revision);assert.equal(s.getSnapshot().slice.entries.length,5);assert.ok(s.getSnapshot().slice.entries.every(e=>e.source==="local"&&e.album.source_id==="disk"));
  const diskEntry=s.getSnapshot().slice.entries[0],diskRevision=s.getSnapshot().revision;
  const nasAlbums=albums.map(a=>({...a,id:`new-${a.id}`,source_id:"nas"}));
  s.syncContext({local:{albums:nasAlbums,state:"ready",sourceId:"nas"}},true);
  assert.equal(s.getSnapshot().slice,null);assert.equal(s.validDetail(diskEntry),false);assert.equal(s.canUseSlice(diskEntry,diskRevision),false);
  const changed=s.getSnapshot();s.openSlice(group,diskRevision);assert.equal(s.getSnapshot(),changed);
  group=changed.genres[0];assert.equal(group.key,groupKey);assert.deepEqual(exploreGenreCardStyle(group.value),style);assert.equal(s.genreAlbumCount(group),6);
  s.openSlice(group,changed.revision);assert.deepEqual(s.getSnapshot().slice.entries.map(e=>e.album),nasAlbums);assert.ok(s.getSnapshot().slice.entries.every(e=>e.source==="local"&&e.album.source_id==="nas"));
  assert.equal(s.getSnapshot().slice.entries.length,s.genreAlbumCount(group));assert.equal(JSON.stringify(albums),before);
});


test("displayed genre colors reserve preferences and resolve collisions by normalized identity, independently of order",()=>{
  const names=["Jazz","Rock","Blues","Pop","Samba","Reggae","Country","Soul","Funk","Ambient","Classical","Electronic"];
  const pair=names.flatMap((a,i)=>names.slice(i+1).filter(b=>exploreGenreCardStyle(a).background===exploreGenreCardStyle(b).background).map(b=>[a,b]))[0];assert.ok(pair);
  const values=[...pair,"Hip Hop","R&B"],colors=assignExploreGenreColors(values);assert.equal(new Set(Object.values(colors)).size,4);
  assert.deepEqual(assignExploreGenreColors([...values].reverse()),colors);
  assert.deepEqual(assignExploreGenreColors(values.map(v=>`  ${v.toUpperCase()}  `)),colors);
  for(let i=0;i<10;i++)assert.deepEqual(assignExploreGenreColors(values),colors);
  assert.equal(assignExploreGenreColors([pair[0]])[pair[0].toLowerCase()],exploreGenreCardStyle(pair[0]).background);
  assert.equal(assignExploreGenreColors(["constructor","__proto__"]).constructor.startsWith("#"),true);
});
