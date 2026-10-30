import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/utils/localCoverBatch.ts',import.meta.url),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};new Function('exports',code)(exports);
const {createLocalCoverBatch}=exports;
test('same-turn covers share bounded requests and retain order',async()=>{
 const calls=[];const read=createLocalCoverBatch(async paths=>{calls.push(paths);return paths.map(path=>({Ok:path}));});
 assert.deepEqual(await Promise.all(['a','b','c','d','e'].map(read)),['a','b','c','d','e']);assert.deepEqual(calls,[['a','b'],['c','d'],['e']]);
});
test('a per-cover refusal preserves its successful sibling',async()=>{
 const read=createLocalCoverBatch(async()=>[{Err:'LOCAL_COVER_OUTSIDE_SOURCE'},{Ok:'image'}]);
 const a=read('a'),b=read('b');await assert.rejects(a,e=>e==='LOCAL_COVER_OUTSIDE_SOURCE');assert.equal(await b,'image');
});
test('whole-snapshot refusal rejects every cover without stranding later requests',async()=>{
 const error=new Error('snapshot changed');let count=0;const read=createLocalCoverBatch(async()=>{if(count++===0)throw error;return [{Ok:null}];});
 const a=read('a'),b=read('b');await Promise.all([assert.rejects(a,e=>e===error),assert.rejects(b,e=>e===error)]);assert.equal(await read('c'),null);
});
test('malformed payload never releases a partial pair',async()=>{
 const read=createLocalCoverBatch(async()=>[{Ok:'image'},{}]);const results=await Promise.allSettled([read('a'),read('b')]);assert.ok(results.every(r=>r.status==='rejected'&&r.reason.message==='LOCAL_COVER_BATCH_INVALID'));
});
