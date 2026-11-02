import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
function compile(file,deps={}){const exports={};new Function('require','exports',ts.transpileModule(readFileSync(new URL('../src/'+file,import.meta.url),'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS}}).outputText)(id=>{assert.ok(id in deps,id);return deps[id]},exports);return exports;}
const helpers=compile('utils/sourceReadiness.ts');
const catalog={albums:[],loaded:true,loading:false,error:null,mpdBusy:false,saveBlocked:false};
test('green Local requires a confirmed catalog, not saved presentation rows, engine availability or non-empty albums',()=>{
 const ready=(state,available=true,updating=false,unavailable=false)=>helpers.localSourceReadiness(true,true,available,unavailable,updating,state);
 assert.equal(ready(catalog),'ready');
 assert.equal(ready({...catalog,loaded:false,presentationAlbums:[{}]}),'checking');
 for(const patch of [{loading:true},{mpdBusy:true},{saveBlocked:true},{manualCheckPlayback:true}])assert.equal(ready({...catalog,...patch}),'busy');
 assert.equal(ready({...catalog,error:'unavailable'}),'error');assert.equal(ready(catalog,false),'checking');assert.equal(ready(catalog,true,true),'busy');assert.equal(ready(catalog,false,false,true),'error');
 assert.equal(helpers.localSourceReadiness(false,true,true,false,false,catalog),'disabled');assert.equal(helpers.localSourceReadiness(true,false,true,false,false,catalog),'unconfigured');
});
test('remote readiness distinguishes unconfigured, previous-account, loading, error and a successful empty collection',()=>{
 assert.equal(helpers.remoteSourceReadiness(false,true,true,false,false),'disabled');assert.equal(helpers.remoteSourceReadiness(true,false,true,false,false),'unconfigured');
 assert.equal(helpers.remoteSourceReadiness(true,true,false,false,false),'checking');assert.equal(helpers.remoteSourceReadiness(true,true,true,true,false),'checking');
 assert.equal(helpers.remoteSourceReadiness(true,true,true,false,true),'error');assert.equal(helpers.remoteSourceReadiness(true,true,true,false,false),'ready');
});
const flush=async()=>{for(let i=0;i<60;i++)await Promise.resolve()};
const defer=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return{resolve,promise}};
function fixture(options={}){
 let host;const calls=[],same=(a,b)=>a?.length===b?.length&&a?.every((v,i)=>Object.is(v,b[i]));
 const hooks={useState(initial){const i=host.cursor++;host.slots[i]??={value:typeof initial==='function'?initial():initial};return[host.slots[i].value,next=>{assert.equal(host.closed,false);host.slots[i].value=typeof next==='function'?next(host.slots[i].value):next}]},
 useMemo(fn,deps){const i=host.cursor++;if(!same(host.slots[i]?.deps,deps))host.slots[i]={deps,value:fn()};return host.slots[i].value},useCallback(fn,deps){return this.useMemo(fn,deps)},
 useEffect(fn,deps){const i=host.cursor++,old=host.slots[i];if(!same(old?.deps,deps))host.effects.push(()=>{old?.cleanup?.();host.slots[i]={deps,cleanup:fn()}})}};
 hooks.useCallback=(fn,deps)=>hooks.useMemo(()=>fn,deps);
 const connection={connection_id:'connection',account_id:'account'};
 const mod=compile('utils/useSourceReadiness.ts',{react:hooks,'./sourceReadiness':helpers,'./musicSources':{sourceEnabled:(cfg,source)=>!!cfg?.sources[source]},'./localAlbumCatalogSession':{localAlbumCatalog:{ensureLoaded:(...args)=>calls.push(['local',...args]),requestRefresh:()=>calls.push(['retry-local'])}},'../services/navidrome':{navidromeService:{status:async()=>{calls.push(['status']);return options.status?options.status():connection},albums:async(...args)=>{calls.push(['albums',...args]);return options.albums?options.albums(...args):{albums:[],offset:0,more:false,total:0}}}}});
 host={slots:[],effects:[],cursor:0,closed:false,input:{config:{first_run:false,sources:{local:true,plex:true,navidrome:true},local_folders:['fixture'],plex_server_id:'server',plex_token:'fixture'},available:true,unavailable:false,updating:false,catalog,plexIdentity:{},plexCurrent:true,plexLoading:false,plexError:false,plexAttempt:0,retryPlex:()=>calls.push(['retry-plex']),navConfigured:true,navRevision:0,...options.input},paint(){for(let n=0;n<3;n++){host.cursor=0;host.effects=[];host.result=mod.useSourceReadiness(host.input);host.effects.splice(0).forEach(fn=>fn())}return host.result},async settle(){await flush();host.paint();await flush();return host.paint()},close(){host.slots.forEach(s=>s?.cleanup?.());host.closed=true}};
 host.paint();return{host,calls};
}
test('disabled sources and the initial wizard perform no readiness I/O',async()=>{
 for(const config of [{first_run:false,sources:{local:false,plex:false,navidrome:false},local_folders:['fixture']},{first_run:true,sources:{local:true,plex:true,navidrome:true},local_folders:['fixture']}]){
 const f=fixture({input:{config}});await f.host.settle();assert.deepEqual(f.calls,[]);assert.deepEqual(Object.values(f.host.result.statuses),['disabled','disabled','disabled']);f.host.close();}
});
test('Navidrome readiness observes one bounded catalog page and rechecks the account before green; retries are explicit',async()=>{
 const f=fixture();assert.equal(f.host.result.statuses.navidrome,'checking');await f.host.settle();assert.equal(f.host.result.statuses.navidrome,'ready');assert.deepEqual(f.calls.filter(c=>c[0]==='albums'),[['albums','connection','','name',0,18]]);
 f.host.result.report('navidrome','error');f.host.paint();assert.equal(f.host.result.statuses.navidrome,'error');f.host.result.retry('navidrome');await f.host.settle();assert.equal(f.host.result.statuses.navidrome,'ready');assert.equal(f.calls.filter(c=>c[0]==='albums').length,2);f.host.close();
});
test('Navidrome failure or a changed account never grants green readiness',async()=>{
 let n=0;for(const options of [{albums:async()=>{throw 'OFFLINE'}},{status:async()=>({connection_id:'c',account_id:++n===1?'one':'two'})}]){
 const f=fixture(options);await f.host.settle();assert.equal(f.host.result.statuses.navidrome,'error');f.host.close();}
});
test('deactivation fences an unfinished readiness request before catalog access or stale green publication',async()=>{
 const wait=defer(),f=fixture({status:()=>wait.promise});await flush();f.host.input={...f.host.input,config:{...f.host.input.config,sources:{local:false,plex:false,navidrome:false}}};f.host.paint();wait.resolve({connection_id:'old',account_id:'old'});await f.host.settle();assert.deepEqual(f.calls.filter(c=>c[0]==='albums'),[]);assert.equal(f.host.result.statuses.navidrome,'disabled');f.host.close();
});

test('a successful library retry clears the source error, while a stale account report cannot grant readiness',async()=>{
 const f=fixture({albums:async()=>{throw 'OFFLINE'}});await f.host.settle();assert.equal(f.host.result.statuses.navidrome,'error');
 f.host.result.report('navidrome','ready');f.host.paint();assert.equal(f.host.result.statuses.navidrome,'ready');
 f.host.result.report('plex','error');f.host.paint();assert.equal(f.host.result.statuses.plex,'error');f.host.result.report('plex','ready');f.host.paint();assert.equal(f.host.result.statuses.plex,'ready');
 const stale=f.host.result.report;f.host.input={...f.host.input,navRevision:1,plexIdentity:{},plexLoading:true};await f.host.settle();
 stale('navidrome','ready');stale('plex','ready');f.host.paint();assert.equal(f.host.result.statuses.navidrome,'error');assert.equal(f.host.result.statuses.plex,'checking');f.host.close();
});
