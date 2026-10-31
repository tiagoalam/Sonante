import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import {renderToStaticMarkup} from 'react-dom/server';
import {PlexCatalogCoordinator} from '../src/utils/plexCatalogCoordinator.ts';
import {plexBrowserLoadState} from '../src/utils/plexBrowserLoadState.ts';
const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');
const ast=ts.createSourceFile('App.tsx',app,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
let handler,notice,contentGate;
function visit(n){
 if(ts.isVariableDeclaration(n)&&n.name.getText(ast)==='retryPlexBrowser')handler=n.initializer.getText(ast);
 if(ts.isVariableDeclaration(n)&&n.name.getText(ast)==='plexLoadBlocksContent')contentGate=n.initializer.getText(ast);
 if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==='PlexBrowserLoadNotice')notice=n;
 ts.forEachChild(n,visit);
}visit(ast);assert.ok(handler);assert.ok(notice);
const js=ts.transpileModule(`const handler=${handler};`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;
const run=s=>new Function(...Object.keys(s),`${js};return handler;`)(...Object.values(s));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const flush=async()=>{for(let i=0;i<35;i++)await Promise.resolve()};
function scope(){
 const identity={},connection={},events=[];
 const s={loadingLibraries:false,plexLibrariesError:false,plexLibrariesIdentity:connection,plexConnectionIdentity:connection,
  selectedLibrary:{key:'1',title:'One'},plexBrowserIdentity:identity,plexBrowserIdentityRef:{current:identity},plexCatalogRetryRef:{current:null},
  plexAlbumLoad:{identity,phase:'error'},plexLibraryRequestGenerationRef:{current:0},plexCatalog:null,albums:[],attempt:0,
  setPlexDiscoveryAttempt(fn){s.attempt=fn(s.attempt)},setPlexAlbumLoad(value){events.push(['load',value.phase]);s.plexAlbumLoad=value},setAlbums(value){events.push(['albums']);s.albums=value},logError(operation){events.push(['error',operation])}};
 return{s,events};
}
const phase=s=>plexBrowserLoadState({loadingLibraries:s.loadingLibraries,librariesError:s.plexLibrariesError,librariesIdentity:s.plexLibrariesIdentity,connectionIdentity:s.plexConnectionIdentity,selectedLibrary:s.selectedLibrary,browserIdentity:s.plexBrowserIdentity,albumLoad:s.plexAlbumLoad});
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!React.isValidElement(tree)?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props.children,p)];
async function view(language){
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(readFileSync(new URL('../src/locales/'+language+'.json',import.meta.url)))}}});
 const deps={react:React,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'lucide-react':icons};
 const exports={};new Function('require','exports',ts.transpileModule(readFileSync(new URL('../src/components/PlexBrowserLoadNotice.tsx',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText)(id=>{assert.ok(id in deps,id);return deps[id]},exports);
 return{Notice:exports.PlexBrowserLoadNotice,t:i18n.t.bind(i18n)};
}
test('Plex browser distinguishes slow requests, discovery failures and confirmed empty libraries, without showing an old account error',()=>{
 const{s}=scope();s.loadingLibraries=true;s.selectedLibrary=null;assert.equal(phase(s),'libraries_loading');
 s.loadingLibraries=false;s.plexLibrariesError=true;assert.equal(phase(s),'libraries_error');
 s.plexLibrariesError=false;assert.equal(phase(s),'no_libraries');
 s.selectedLibrary={key:'1'};s.plexAlbumLoad=null;assert.equal(phase(s),'catalog_loading');
 s.plexAlbumLoad={identity:s.plexBrowserIdentity,phase:'ready'};assert.equal(phase(s),null);
 s.plexAlbumLoad={identity:{},phase:'error'};assert.equal(phase(s),'catalog_loading');
 s.plexLibrariesError=true;s.plexConnectionIdentity={};assert.equal(phase(s),'libraries_loading');
});
test('explicit retry shares Discover library-discovery trigger and does nothing while that request is pending',async()=>{
 for(const mode of ['error','empty','context']){
  const{s}=scope();s.plexCatalog={loadLibrary:()=>assert.fail('discovery must precede album loading')};
  if(mode==='error')s.plexLibrariesError=true;else if(mode==='empty')s.selectedLibrary=null;else s.plexLibrariesIdentity={};
  await run(s)();assert.equal(s.attempt,1);s.loadingLibraries=true;await run(s)();assert.equal(s.attempt,1);
 }
});
test('real failed catalog becomes loading on click and retries only the selected library; duplicate clicks and renders do not query again',async()=>{
 const{s}=scope(),calls=[];const c=new PlexCatalogCoordinator(key=>{const d=deferred();calls.push({key,...d});return d.promise});c.setContext(s.plexConnectionIdentity,'server');s.plexCatalog=c;
 const failed=c.loadLibrary(s.selectedLibrary).catch(()=>{}),other=c.loadLibrary({key:'2',title:'Two'});await flush();calls[0].reject(Error('fixture transport failure'));calls[1].resolve([{rating_key:'kept',title:'Kept',artist:'Artist'}]);await Promise.all([failed,other]);await flush();
 const known=c.getSnapshot().libraries.find(l=>l.library.key==='2').albums;const retry=run(s),pending=retry();assert.equal(phase(s),'catalog_loading');await retry();
 await flush();assert.deepEqual(calls.map(c=>c.key),['1','2','1']);for(let i=0;i<10;i++)assert.equal(phase(s),'catalog_loading');assert.equal(calls.length,3);
 calls[2].resolve([{rating_key:'new',title:'New',artist:'Artist'}]);await pending;assert.equal(phase(s),null);assert.equal(s.albums[0].rating_key,'new');assert.equal(s.albums[0].library_id,'1');assert.equal(c.getSnapshot().libraries.find(l=>l.library.key==='2').albums,known);assert.equal(s.plexCatalogRetryRef.current,null);c.invalidate();
});
test('a failed explicit retry keeps confirmed same-context data and exposes another retry rather than an empty success',async()=>{
 const{s}=scope(),d=deferred();const kept=[{rating_key:'kept'}];s.albums=kept;s.plexCatalog={loadLibrary:()=>d.promise};const p=run(s)();assert.equal(s.albums,kept);d.reject(Error('PRIVATE backend detail'));await p;assert.equal(phase(s),'catalog_error');assert.equal(s.albums,kept);assert.equal(s.plexCatalogRetryRef.current,null);
});
test('late success/error cannot overwrite a different library/account, and an old finally cannot clear a newer retry',async()=>{
 for(const fail of [false,true]){
  const{s,events}=scope(),a=deferred(),b=deferred();let calls=0;s.plexCatalog={loadLibrary:()=>++calls===1?a.promise:b.promise};
  const first=run(s)();const next={};s.plexBrowserIdentity=next;s.plexBrowserIdentityRef.current=next;s.selectedLibrary={key:'2'};s.plexAlbumLoad={identity:next,phase:'error'};const second=run(s)();assert.equal(s.plexCatalogRetryRef.current,next);
  const before=events.length;if(fail)a.reject(Error('OLD PRIVATE'));else a.resolve([{rating_key:'stale'}]);await first;assert.equal(events.length,before);assert.equal(s.plexCatalogRetryRef.current,next);
  b.resolve([{rating_key:'current'}]);await second;assert.equal(s.albums[0].rating_key,'current');assert.equal(s.plexCatalogRetryRef.current,null);
 }
 const{s,events}=scope(),a=deferred();s.plexCatalog={loadLibrary:()=>a.promise};const p=run(s)();s.plexBrowserIdentityRef.current={};const before=events.length;a.resolve([{rating_key:'old-account'}]);await p;assert.equal(events.length,before);
});
for(const language of ['en','pt-BR'])test(`real notice has a loading status and an actionable, localized error without raw backend data (${language})`,async()=>{
 const{Notice,t}=await view(language);let clicks=0;
 for(const state of ['libraries_loading','libraries_error','no_libraries','catalog_loading','catalog_error']){
  const tree=Notice({state,compact:false,onRetry:()=>clicks++}),loading=state.endsWith('_loading');
  assert.equal(tree.props['aria-busy'],loading);assert.equal(tree.props.role,state.endsWith('_error')?'alert':'status');
  const buttons=nodes(tree,n=>n.type==='button');assert.equal(buttons.length,loading?0:1);
  if(!loading){assert.ok(renderToStaticMarkup(tree).includes(t('plex.retryLoad')));buttons[0].props.onClick()}
  assert.doesNotMatch(renderToStaticMarkup(tree),/plex\.(?:loading|catalog|libraries|retry|noLibraries)|PRIVATE/);
 }
 assert.equal(clicks,3);assert.equal(Notice({state:null,compact:true,onRetry(){}}),null);
});
test('the real browser retry invokes its handler, while confirmed inventory and search results remain visible with a notice',async()=>{
 const callback=notice.attributes.properties.find(p=>p.name?.getText(ast)==='onRetry').initializer.expression.getText(ast);let called=0;new Function('retryPlexBrowser',`return (${callback});`)(()=>{called++})();assert.equal(called,1);
 const blocked=new Function('plexLoadState','selectedLibrary','albums','searchQuery',`return (${contentGate});`);
 for(const state of ['catalog_loading','catalog_error']) {
  assert.equal(blocked(state,{key:'1'},[],''),true);
  assert.equal(blocked(state,{key:'1'},[{rating_key:'confirmed'}],''),false);
  assert.equal(blocked(state,{key:'1'},[],'search in progress'),false);
 }
 assert.equal(blocked('libraries_loading',null,[{rating_key:'old-account'}],'query'),true);
});
