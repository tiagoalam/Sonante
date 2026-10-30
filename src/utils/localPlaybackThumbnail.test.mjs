import test from "node:test";
import assert from "node:assert/strict";
import { LocalPlaybackThumbnail, LOCAL_PLAYBACK_THUMBNAIL_KEY, MAX_PLAYBACK_THUMBNAIL_RECORD } from "./localPlaybackThumbnail.ts";
const jpeg = (width=80) => "data:image/jpeg;base64," + Buffer.from([255,216,255,192,0,11,8,0,80,width>>8,width&255,1,1,17,0,255,217]).toString("base64");
const media = uri => ({kind:"local",uri,queue_index:0});
const track = media("Source/Album/track.flac");
const image = () => ({src:"data:image/png;base64,AAAA",complete:true,naturalWidth:80,naturalHeight:80});
function fixture() {
 let value=null,writes=0,reads=0,reductions=0;
 const storage={getItem(key){assert.equal(key,LOCAL_PLAYBACK_THUMBNAIL_KEY);reads++;return value},setItem(key,next){assert.equal(key,LOCAL_PLAYBACK_THUMBNAIL_KEY);writes++;value=next},removeItem(){value=null}};
 const cache=new LocalPlaybackThumbnail(()=>storage,()=>{reductions++;return jpeg()});
 cache.setContext(["/music"]);
 return {cache,storage,get value(){return value},set value(v){value=v},get writes(){return writes},get reads(){return reads},get reductions(){return reductions}};
}
test("one loaded image is restored synchronously without reads or reductions on render",()=>{
 const f=fixture();f.cache.capture(track,image(),()=>true);
 const restored=new LocalPlaybackThumbnail(()=>f.storage,()=>assert.fail("no decode on restore"));
 restored.setContext(["/music"]);const reads=f.reads,writes=f.writes;
 for(let n=0;n<20;n++)assert.equal(restored.get(track),jpeg());
 assert.equal(f.reads,reads);assert.equal(f.writes,writes);
 assert.equal(restored.get(media("Source/Other/track.flac")),null);
 assert.equal(restored.get({kind:"plex",server_id:"server",part_key:"part",queue_index:0}),null);
 restored.setContext(["/other"]);assert.equal(restored.get(track),null);
 restored.setContext([]);assert.equal(restored.get(track),null);
});
test("track changes with the same decoded image update the one entry; polling never recaptures it",()=>{
 const f=fixture(),img=image();f.cache.capture(track,img,()=>true);
 for(let n=0;n<20;n++)f.cache.capture(track,img,()=>true);
 assert.equal(f.reductions,1);assert.equal(f.writes,1);
 const second=media("Source/Album/two.flac");f.cache.capture(second,img,()=>true);
 assert.equal(f.reductions,2);assert.equal(f.cache.get(track),null);assert.equal(f.cache.get(second),jpeg());
 f.cache.capture(second,{...img,src:jpeg()},()=>true);assert.equal(f.reductions,2,"our preview is never recaptured");
});
test("invalid cached content, contexts, remote sources and unsafe media never become a player preview",()=>{
 const f=fixture();f.cache.capture(track,image(),()=>true);const good=JSON.parse(f.value);
 const bad=["{","null","x".repeat(MAX_PLAYBACK_THUMBNAIL_RECORD+1),
  ...[{version:2},{context:'["/other"]'},{thumb:jpeg(4096)},{thumb:"https://server/secret"},{thumb:"data:image/svg+xml;base64,AAAA"},{uri:"../track.flac"},{uri:"https://server/token"}].map(change=>JSON.stringify({...good,...change}))];
 for(const record of bad){f.value=record;const next=new LocalPlaybackThumbnail(()=>f.storage);next.setContext(["/music"]);assert.equal(next.get(track),null)}
 for(const uri of ["../track.flac","Source/../track.flac","Source//track.flac","/Source/track.flac","Source/track.flac\n","https://server/token","track.flac"]){
  f.cache.capture(media(uri),image(),()=>assert.fail("unsafe URI cannot reach image/storage work"));assert.equal(f.cache.get(media(uri)),null);
 }
 const actual=new LocalPlaybackThumbnail(()=>f.storage);actual.setContext(["/music"]);
 for(const src of ["https://server/token","file:///cover","asset:///cover","data:image/svg+xml;base64,AAAA"]){actual.capture(track,{...image(),src},()=>true);assert.equal(actual.get(track),null)}
});
test("stale capture, storage failure and an undecodable preview do not block playback or revive another image",()=>{
 const f=fixture();f.cache.capture(track,image(),()=>false);assert.equal(f.writes,0);
 let cache;cache=new LocalPlaybackThumbnail(()=>f.storage,()=>{cache.setContext(["/changed"]);return jpeg()});cache.setContext(["/music"]);cache.capture(track,image(),()=>true);assert.equal(cache.get(track),null);assert.equal(f.writes,0);
 const denied=new LocalPlaybackThumbnail(()=>{throw Error("private storage error")});assert.doesNotThrow(()=>denied.setContext(["/music"]));assert.equal(denied.get(track),null);
 f.cache.capture(track,image(),()=>true);f.cache.forget(track,"another-source");assert.equal(f.cache.get(track),jpeg());f.cache.forget(track,jpeg());assert.equal(f.cache.get(track),null);assert.equal(f.value,null);
});
