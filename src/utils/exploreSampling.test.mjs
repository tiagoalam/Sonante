import test from 'node:test';
import assert from 'node:assert/strict';
import {pickExploreSuggestions} from './explore.ts';
import {referencePickExploreSuggestions} from '../../tests/helpers/exploreSamplingReference.mjs';

function rng(seed) {
 let draws=0;
 const random=()=>{draws++;seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/2**32;};
 random.draws=()=>draws;return random;
}
function pool(size) {
 const version=Array.from({length:size},(_,i)=>({id:'a'+i,title:'Album '+(i%7===0?i-1:i),
  artist:['Artist '+(i%Math.max(1,Math.floor(size/10))),'BJO\u0308RK','Björk','Unknown','Various Artists','Nìna','Nina'][i%7],
  year:[undefined,1972,'1972-01-02',2000,'bad','1972-99-99'][i%6],genres:[i%2?'Rock':'Jazz'],
  source_id:'source'+i%2,folder_path:'Generated/'+i,discs:[],track_count:10}));
 return version.map(album=>({key:JSON.stringify(['local',album.id]),source:'local',album,version}));
}
function compare(entries,retained,seed,preserve) {
 const a=rng(seed),b=rng(seed);
 const expected=referencePickExploreSuggestions(entries,retained,a,preserve);
 const actual=pickExploreSuggestions(entries,retained,b,preserve);
 assert.deepEqual(actual,expected);
 actual.forEach((entry,i)=>assert.equal(entry,expected[i]));
 assert.equal(a.draws(),b.draws());
}

test('Explore scoring preserves the previous exact picks, tie order, retained copies and random stream',()=>{
 for(const size of [0,1,4,10,30,80])for(const seed of [0,1,17,99])for(const preserve of [false,true]){
  const entries=pool(size),stale={...entries[0],key:'stale'};
  const before=JSON.stringify(entries);
  for(const kept of [[],entries.slice(0,2),entries.slice(0,10),[...entries.slice(0,12),stale]])
   compare(entries,kept,seed,preserve);
  assert.equal(JSON.stringify(entries),before);
 }
});

test('Explore scoring preserves missing-year copies, editions, generic artists and retained representatives',()=>{
 const entries=pool(40);
 for(const [i,artist] of [undefined,null,42,'', '  ', '???','Artist A','Artist A','Artist A','Artist A'].entries()){
  entries[i].album.artist=artist;
  entries[i].album.title=i<6?'Unknown album':'Same album';
  entries[i].album.year=[undefined,1972,2000,undefined][i%4];
 }
 const duplicate={...entries[6],key:'another-source',album:{...entries[6].album,source_id:'another-source'}};
 entries.push(duplicate);
 for(const retained of [[],[duplicate],[entries[8],duplicate,entries[6]],entries.slice(0,10)])
  for(const preserve of [false,true])for(const seed of [0,1,17])compare(entries,retained,seed,preserve);
});

test('Explore artist keys are reused within selection and never across metadata or catalog changes',()=>{
 const entries=pool(250);let reads=0;
 entries.forEach((entry,i)=>Object.defineProperty(entry.album,'artist',{configurable:true,get(){reads++;return 'Artist '+i%25;}}));
 const a=rng(17),b=rng(17);
 const expected=referencePickExploreSuggestions(entries,[],a);const previousReads=reads;reads=0;
 const actual=pickExploreSuggestions(entries,[],b);
 assert.deepEqual(actual,expected);assert.equal(a.draws(),b.draws());
 assert.ok(reads<=2*entries.length+10,`metadata reads should be linear: ${reads}`);
 assert.ok(previousReads>reads*10,'fixture exercises the former repeated-normalization path');
 entries.forEach(entry=>Object.defineProperty(entry.album,'artist',{configurable:true,value:'Changed artist'}));
 compare(entries,[],17,false);
 const revised=pool(250);
 revised.forEach((entry,i)=>{entry.album.artist='New '+i%3;});
 compare(revised,actual,17,false);compare(revised,actual,17,true);
});
