import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/utils/localSourceReadQueue.ts',import.meta.url),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};new Function('exports',code)(exports);
const {createLocalSourceReadQueue}=exports;
function deferred(){let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject};}
const settle=()=>new Promise(resolve=>setImmediate(resolve));
test('catalog, covers and availability cannot overlap their native read gate',async()=>{
 const run=createLocalSourceReadQueue(),first=deferred(),second=deferred(),calls=[];
 const a=run(()=>{calls.push('catalog');return first.promise;});
 const b=run(()=>{calls.push('cover');return second.promise;});
 const c=run(()=>{calls.push('availability');return Promise.resolve('observed');});
 await settle();assert.deepEqual(calls,['catalog']);first.resolve(['album']);assert.deepEqual(await a,['album']);
 await settle();assert.deepEqual(calls,['catalog','cover']);second.resolve('cover-data');assert.equal(await b,'cover-data');
 assert.equal(await c,'observed');assert.deepEqual(calls,['catalog','cover','availability']);
});
test('a refused read stays refused and does not strand the next read',async()=>{
 const run=createLocalSourceReadQueue(),failure=new Error('SOURCE_AVAILABILITY_CHANGED');
 const a=run(()=>Promise.reject(failure));const b=run(()=>Promise.resolve('next'));
 await assert.rejects(a,error=>error===failure);assert.equal(await b,'next');
});
test('a synchronous boundary exception is preserved and the gate is released',async()=>{
 const run=createLocalSourceReadQueue(),failure=new Error('native unavailable');
 const a=run(()=>{throw failure;});const b=run(()=>Promise.resolve(null));
 await assert.rejects(a,error=>error===failure);assert.equal(await b,null);
});
