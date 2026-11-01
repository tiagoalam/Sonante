import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import * as jsx from 'react/jsx-runtime';
const read=p=>readFileSync(new URL('../src/'+p,import.meta.url),'utf8');
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(options={}){
 let cursor=0,closed=false,props;const slots=[],effects=[],calls=[],opened=[];
 const react={useState(initial){const i=cursor++;if(!(i in slots))slots[i]=initial;return[slots[i],v=>{assert.equal(closed,false,'no state after unmount');slots[i]=typeof v==='function'?v(slots[i]):v}]},useRef(initial){return slots[cursor++]??={current:initial}},useEffect(fn,deps){const i=cursor++,old=slots[i];if(!old||deps.some((d,j)=>!Object.is(d,old.deps[j])))effects.push(()=>{old?.cleanup?.();slots[i]={deps,cleanup:fn()}})}};
 const exports={},deps={react,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:key=>key})},'../services/navidrome':{navidromeService:{album:async(...args)=>{calls.push(args);return options.album?options.album(...args):{album:{id:props.albumId},artist_id:'artist-1'}}},navidromeError:()=> 'unavailable'}};
 new Function('require','exports',ts.transpileModule(read('components/NavidromeArtistLink.tsx'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);
 props={artist:'Same artist',connectionId:'session',accountId:'a'.repeat(64),albumId:'album-1',onOpen:target=>opened.push(target)};
 const render=(next={})=>{props={...props,...next};cursor=0;const tree=exports.NavidromeArtistLink(props);effects.splice(0).forEach(fn=>fn());return tree;};
 return{render,calls,opened,button:tree=>nodes(tree,n=>n.type==='button')[0],close(){slots.forEach(s=>s?.cleanup?.());closed=true;}};
}
test('Navidrome artist links do no catalog work before click and open the exact album artist once on double click',async()=>{
 const pending=deferred(),f=fixture({album:()=>pending.promise});try{const tree=f.render();assert.deepEqual(f.calls,[]);const button=f.button(tree);button.props.onClick();button.props.onClick();assert.deepEqual(f.calls,[['session','album-1']]);pending.resolve({album:{id:'album-1'},artist_id:'artist-exact'});await tick();assert.deepEqual(f.opened,[{account_id:'a'.repeat(64),id:'artist-exact'}]);}finally{f.close()}
});
test('artist link missing or wrong identity remains an actionable sanitized error, without guessing by name',async()=>{
 for(const response of [{album:{id:'album-1'},artist_id:null},{album:{id:'another'},artist_id:'artist-1'}]){const f=fixture({album:async()=>response});try{f.button(f.render()).props.onClick();await tick();assert.deepEqual(f.opened,[]);assert.equal(nodes(f.render(),n=>n.props?.role==='alert').length,1);assert.equal(f.button(f.render()).props.disabled,false);}finally{f.close()}}
});
test('old artist-link responses cannot navigate after a changed account, card, connection, disabled state or unmount',async()=>{
 for(const change of [{connectionId:'next'},{accountId:'b'.repeat(64)},{albumId:'next'},{disabled:true},null]){const response=deferred(),f=fixture({album:()=>response.promise});f.button(f.render()).props.onClick();if(change)f.render(change);else f.close();response.resolve({album:{id:'album-1'},artist_id:'artist-1'});await tick();assert.deepEqual(f.opened,[]);if(change)f.close();}
});
test('Navidrome player-navigation RPC is an explicit readonly request and never invokes playback preparation',async()=>{
 const calls=[],exports={};const deps={'@tauri-apps/api/core':{invoke:async(...args)=>{calls.push(args);return{}}},'./config':{ensureDirectOutputChoice:()=>assert.fail()},'../utils/localSourceReadQueue':{localSourceReadQueue:{}},'../utils/playbackIntent':{playbackIntentToken:()=>assert.fail()}};
 new Function('require','exports',ts.transpileModule(read('services/navidrome.ts'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText)(n=>{assert.ok(n in deps,n);return deps[n]},exports);
 await exports.navidromeService.resolvePlayerNavigation({connection_id:'session',song_id:'song',queue_index:2});assert.deepEqual(calls,[['navidrome_player_navigation',{connectionId:'session',songId:'song'}]]);
});
