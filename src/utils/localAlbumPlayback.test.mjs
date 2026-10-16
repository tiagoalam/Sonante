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

test("one pending admission across cards includes load, artwork and playback acknowledgement",async()=>{
  const load=deferred(),cover=deferred(),ack=deferred();let reads=0,covers=0,plays=0;
  const p=createLocalAlbumPlayback(()=>{reads++;return load.promise;},()=>{covers++;return cover.promise;},()=>{plays++;return ack.promise;});
  const run=p.play(album,()=>true);await p.play({...album,id:"other"},()=>true);assert.equal(reads,1);
  load.resolve(groups);await flush();await p.play(album,()=>true);assert.equal(covers,1);assert.equal(reads,1);
  cover.resolve(null);await flush();await p.play(album,()=>true);assert.equal(plays,1);assert.equal(reads,1);
  ack.resolve();await run;await p.play(album,()=>true);assert.equal(plays,2);
});

test("invalid context suppresses initial reads and each later stage; cancellation survives leaving and returning",async()=>{
  for(const stage of ["initial","load","cover","cancel"]){
    const d=deferred();let allowed=stage!=="initial",reads=0,covers=0,plays=0;
    const p=createLocalAlbumPlayback(()=>{reads++;return stage==="load"||stage==="cancel"?d.promise:Promise.resolve(groups);},()=>{covers++;return stage==="cover"?d.promise:Promise.resolve(null);},async()=>{plays++;});
    const run=p.play(album,()=>allowed);await flush();
    if(stage==="cancel"){p.cancel();await p.play(album,()=>true);assert.equal(reads,1,"cancel does not release a pending read");}
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
