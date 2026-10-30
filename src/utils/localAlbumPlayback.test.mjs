import test from "node:test";
import assert from "node:assert/strict";
import { createLocalAlbumPlayback } from "./localAlbumPlayback.ts";

const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise,resolve,reject}; };
const album = {id:"root-a/exact-edition",source_id:"root-a",title:"Album",artist:"Artist"};
const groups = [
  {disc:{number:1},tracks:[{path:'root-a/Album/CD1/01.flac',name:"01",title:"First",duration:12}, {path:'root-a/Album/CD1/02.flac',name:"02"}]},
  {disc:{number:2},tracks:[{path:'root-a/Album/CD2/disc.cue/track0001',name:"CUE1",artist:"Guest",album:"Tagged"}, {path:'root-a/Album/CD2/disc.cue/track0002',name:"CUE2"}]},
];
const flush = async () => { for(let i=0;i<8;i++)await Promise.resolve(); };

test("Local album playback uses the exact source album ID, backend disc/CUE order and existing metadata/URIs",async()=>{
  const reads=[],plays=[];
  const p=createLocalAlbumPlayback(async id=>{reads.push(id);return groups;},async a=>{assert.equal(a,album);return "fixture-cover";},async(...args)=>plays.push(args));
  await p.play(album,()=>true);
  assert.deepEqual(reads,[album.id]);assert.equal(plays.length,1);assert.equal(plays[0][1],0);
  assert.deepEqual(plays[0][0].map(t=>t.uri),groups.flatMap(g=>g.tracks.map(t=>t.path)));
  assert.deepEqual(plays[0][0].map(t=>[t.title,t.artist,t.album]),[["First","Artist","Album"],["02","Artist","Album"],["CUE1","Guest","Tagged"],["CUE2","Artist","Album"]]);
  assert.ok(plays[0][0].every(t=>t.thumb==="fixture-cover"));assert.equal(plays[0][0][0].duration,12);
});

test("a newer Local album intent supersedes a slow old preparation",async()=>{
  const first=deferred(),second=deferred(),played=[];let reads=0;
  const p=createLocalAlbumPlayback(()=>++reads===1?first.promise:second.promise,async()=>null,async tracks=>played.push(tracks[0].title));
  const a=p.play(album,()=>true),b=p.play({...album,id:"new"},()=>true);
  second.resolve(groups);await b;first.resolve(groups);await a;
  assert.equal(reads,2);assert.deepEqual(played,["First"]);
});

test("invalid context suppresses initial reads and each later stage; cancellation survives leaving and returning",async()=>{
  for(const stage of ["initial","load","cover","cancel"]){
    const d=deferred();let allowed=stage!=="initial",reads=0,covers=0,plays=0;
    const p=createLocalAlbumPlayback(()=>{reads++;return stage==="load"||stage==="cancel"?d.promise:Promise.resolve(groups);},()=>{covers++;return stage==="cover"?d.promise:Promise.resolve(null);},async()=>{plays++;});
    const run=p.play(album,()=>allowed);await flush();
    if(stage==="cancel"){p.cancel();assert.equal(reads,1,"cancel does not replay or stop the active selection");}
    else allowed=false;
    d.resolve(stage==="cover"?null:groups);await run;
    assert.equal(plays,0);assert.equal(reads,stage==="initial"?0:1);assert.equal(covers,stage==="cover"?1:0);
  }
});

test("load, artwork and submission failures propagate once and release admission for an explicit retry",async()=>{
  for(const stage of ["load","cover","play"]){
    let fail=true,plays=0;
    const error=Error("private fixture diagnostic");
    const p=createLocalAlbumPlayback(async()=>{if(fail&&stage==="load")throw error;return groups;},async()=>{if(fail&&stage==="cover")throw error;return null;},async()=>{plays++;if(fail&&stage==="play")throw error;});
    await assert.rejects(p.play(album,()=>true),e=>e===error);fail=false;await p.play(album,()=>true);
    assert.equal(plays,stage==="play"?2:1);
  }
});

test("empty album never requests artwork or playback",async()=>{
  const p=createLocalAlbumPlayback(async()=>[],async()=>assert.fail("artwork"),async()=>assert.fail("play"));
  await p.play(album,()=>true);
});
