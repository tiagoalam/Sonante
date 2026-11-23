import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../src/utils/opraSearch.ts',import.meta.url),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};new Function('require','exports',code)(()=>({}),exports);
const product=(product_name,vendor_name='HIFIMAN')=>({product_id:product_name,vendor_id:'hifiman',product_name,vendor_name,product_type:'headphones',subtype:'over_the_ear',profile_count:1,profiles:[]});

test('OPRA results split full manufacturer/model matches from related matches without reordering',()=>{
 const products=[product('Edition XS'),product('Edition X'),product('Sundara')];
 const result=exports.classifyOpraProducts('HIFIMAN Edition-XS',products);
 assert.deepEqual(result.direct.map(item=>item.product_name),['Edition XS']);
 assert.deepEqual(result.related.map(item=>item.product_name),['Edition X','Sundara']);
 assert.deepEqual(exports.classifyOpraProducts('edition xs',products).direct.map(item=>item.product_name),['Edition XS']);
 assert.deepEqual(exports.classifyOpraProducts('hifiman',products).direct.map(item=>item.product_name),['Edition XS','Edition X','Sundara']);
});
