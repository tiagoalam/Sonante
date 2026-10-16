import test from "node:test";
import assert from "node:assert/strict";
import { albumShelfLayout, albumShelfEdges, albumShelfTarget, ALBUM_SHELF_GAP } from "./exploreAlbumShelf.ts";
import { EXPLORE_PAGE_SIZE, EXPLORE_SUGGESTION_COUNT, pickExploreSuggestions } from "./explore.ts";

const scrollWidth=(layout,count)=>count*layout.cardWidth+Math.max(0,count-1)*ALBUM_SHELF_GAP;
test("shelf measures its content box, keeps comfortable cards and counts fully visible slots",()=>{
  for(const width of [160,350,500,700,1000,1700,2500]){
    const layout=albumShelfLayout(width,10);
    assert.ok(layout.cardWidth>0&&layout.cardWidth<=220);assert.ok(layout.cardWidth<=width);
    assert.ok(layout.visibleCount>=1&&layout.visibleCount<=10);
    assert.ok(layout.visibleCount*layout.cardWidth+(layout.visibleCount-1)*ALBUM_SHELF_GAP<=width+0.01);
  }
  assert.equal(albumShelfLayout(1000,10).visibleCount,5);
  assert.ok(albumShelfLayout(1700,10).visibleCount>6);
  assert.equal(albumShelfLayout(350,10).cardWidth,220,"a near-breakpoint single slot never stretches its cover across the viewport");
  assert.equal(albumShelfLayout(1000,3).cardWidth,220,"short inventory keeps comfortable card size");
});

test("arrows advance an aligned visible group, clamp at both ends and disable without overflow",()=>{
  const width=1000,layout=albumShelfLayout(width,10),total=scrollWidth(layout,10),maximum=total-width;
  assert.deepEqual(albumShelfEdges(width,total,0),{previous:false,next:true});
  const next=albumShelfTarget(width,total,0,layout,1);assert.equal(next,maximum);
  assert.deepEqual(albumShelfEdges(width,total,next),{previous:true,next:false});
  assert.equal(albumShelfTarget(width,total,next,layout,1),maximum);
  assert.equal(albumShelfTarget(width,total,next,layout,-1),0);
  assert.equal(albumShelfTarget(width,total,10,layout,"first"),0);assert.equal(albumShelfTarget(width,total,0,layout,"last"),maximum);
  assert.deepEqual(albumShelfEdges(1000,scrollWidth(albumShelfLayout(1000,3),3),0),{previous:false,next:false});
  const single=albumShelfLayout(350,10);
  assert.equal(albumShelfTarget(350,scrollWidth(single,10),scrollWidth(single,10)-350,single,-1),8*single.stride,"previous at a partial end must not skip the preceding full card");
  const narrow=albumShelfLayout(500,10),end=scrollWidth(narrow,10)-500;
  for(const left of [-10,0,125,end,end+100])for(const direction of [-1,1]){
    const target=albumShelfTarget(500,end+500,left,narrow,direction);assert.ok(target>=0&&target<=end);
    assert.ok(Math.abs(target/narrow.stride-Math.round(target/narrow.stride))<1e-8||target===end);
  }
});

test("ten distinct presentation groups, Local preference, insufficient inventory and slice size stay independent",()=>{
  const local=Array.from({length:12},(_,i)=>({key:`L${i}`,source:"local",album:{id:`L${i}`,title:`Album ${i}`,artist:`Artist ${i}`}}));
  const plex=local.flatMap((e,i)=>Array.from({length:4},(_,j)=>({key:`P${i}-${j}`,source:"plex",album:{rating_key:`P${i}-${j}`,title:e.album.title,artist:e.album.artist}})));
  const selected=pickExploreSuggestions([...local,...plex],[],()=>0.4);
  assert.equal(EXPLORE_SUGGESTION_COUNT,10);assert.equal(EXPLORE_PAGE_SIZE,6);
  assert.equal(selected.length,10);assert.equal(new Set(selected.map(e=>e.album.title)).size,10);assert.ok(selected.every(e=>e.source==="local"));
  assert.equal(pickExploreSuggestions([local[0],...plex.slice(0,4)],[],()=>0.4).length,1);
});
