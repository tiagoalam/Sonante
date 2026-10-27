import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync,existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {dirname,resolve,extname} from 'node:path';
import {createRequire} from 'node:module';
import ts from 'typescript';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import * as icons from 'lucide-react';
import i18next from 'i18next';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'../..');
const nativeRequire=createRequire(import.meta.url);
const equal=(a,b)=>a?.length===b?.length&&a.every((v,i)=>Object.is(v,b[i]));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
let host,language;
const hooks={
 useState(initial){const h=host,index=h.cursor++,slot=h.slots[index]??={value:typeof initial==='function'?initial():initial};return[slot.value,value=>{h.writes++;slot.value=typeof value==='function'?value(slot.value):value;}];},
 useRef(initial){return host.slots[host.cursor++]??={current:initial};},
 useMemo(fn,deps){const h=host,index=h.cursor++,old=h.slots[index];if(!old||!equal(old.deps,deps))h.slots[index]={deps,value:fn()};return h.slots[index].value;},
 useEffect(fn,deps){const h=host,index=h.cursor++,old=h.slots[index];if(!old||!equal(old.deps,deps))h.effects.push(()=>{old?.cleanup?.();h.slots[index]={deps,cleanup:fn()};});},
 useSyncExternalStore(_subscribe,get){host.cursor++;return get();},
};
let snapshot,confirmedRevision,requests,logs,coverRead;
const album={id:'album-a',title:'Fixture album',artist:'Synthetic artist',folder_path:'Synthetic',discs:[{number:1,label:'Disc 1',folder_path:'Synthetic',track_count:1}],track_count:1,source_id:null};
const track={name:'fixture.m4a',path:'Synthetic/fixture.m4a',title:'Fixture track',artist:'Synthetic artist',album:'Fixture album',item_type:'file',duration:1};
const groups=[{disc:album.discs[0],tracks:[track]}];
const catalog={subscribe:()=>()=>{},getSnapshot:()=>snapshot,getConfirmedRevision:()=>confirmedRevision,setUpdating(){},requestRefresh(){},waitForMpd(){}};
const audioService={playTracks:(...args)=>{const d=deferred();requests.push({...d,args});return d.promise;},getLocalAlbumTracks:async()=>groups,resolveLocalLibraryPath:async()=>'/synthetic/folder',listLocalDirectory:async()=>[track,{name:'Subfolder',path:'Synthetic/Subfolder',item_type:'directory'}],getLocalCover:async()=>null};
const artwork={get:()=>null,subscribe:()=>()=>{},resolveLocalOnly:(...a)=>coverRead(...a),resolve:async()=>null};
const replacements={
 react:{__esModule:true,...React,...hooks,default:React},'react/jsx-runtime':await import('react/jsx-runtime'),'react-i18next':{useTranslation:()=>({t:language.t.bind(language),i18n:language})},'lucide-react':icons,
 '../services/audio':{audioService},'../services/favorites':{favoritesService:{getFavorites:async()=>[]}},
 '../utils/frontendDiagnostics':{logError:(op,error)=>logs.push({op,error})}, './frontendDiagnostics':{logError:(op,error)=>logs.push({op,error})},
 '../utils/localAlbumCatalogSession':{localAlbumCatalog:catalog},
 '../utils/localArtworkSession':{localArtworkResolver:artwork,localArtworkEnrichment:{subscribe:()=>()=>{},getSnapshot:()=>({active:0,queued:0,inFlight:0,blocked:false}),setCatalog(){}}},
};
const cache=new Map();
function load(path){
 if(cache.has(path))return cache.get(path);
 const module={};cache.set(path,module);
 const source=readFileSync(path,'utf8');
 const js=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText;
 const localRequire=name=>{
  if(name in replacements)return replacements[name];
  if(!name.startsWith('.'))return nativeRequire(name);
  const base=resolve(dirname(path),name);const file=[base,base+'.ts',base+'.tsx',base+'.js'].find(p=>existsSync(p));
  assert.ok(file,`dependency ${name}`);
  if(extname(file)==='.tsx')return new Proxy({},{get:()=>()=>null});
  return load(file);
 };
 new Function('require','exports',js)(localRequire,module);return module;
}
const {LocalBrowserView}=load(resolve(root,'src/components/LocalBrowserView.tsx'));
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
const markup=tree=>renderToStaticMarkup(tree);
class Harness{
 slots=[];cursor=0;writes=0;effects=[];
 constructor(props){this.props=props;}
 render(props=this.props){this.props=props;this.cursor=0;this.effects=[];host=this;const tree=LocalBrowserView(props);host=null;this.tree=tree;this.effects.splice(0).forEach(fn=>fn());return tree;}
 async settle(){for(let i=0;i<3;i++){this.render();await flush();}return this.render();}
 close(){this.slots.forEach(s=>s?.cleanup?.());}
 click(label){const button=nodes(this.tree,n=>n.type==='button'&&markup(n).includes(label))[0];assert.ok(button,`button ${label}`);return button.props.onClick();}
 alerts(){return nodes(this.render(),n=>n.props?.role==='alert').map(markup);}
 async folders(){this.render({...this.props,initialAlbumId:undefined});this.click(language.t('localBrowser.back'));await this.settle();this.click(language.t('localBrowser.tabFolders'));await this.settle();}
 folderClick(){const row=nodes(this.tree,n=>n.type==='div'&&n.props?.onClick&&n.props?.className?.includes('justify-between')&&markup(n).includes('Fixture track'))[0];assert.ok(row,'folder file row');return row.props.onClick();}
}
function setup(){
 requests=[];logs=[];coverRead=async()=>null;confirmedRevision=1;snapshot={albums:[album],loaded:true,loading:false,saveBlocked:false,mpdBusy:false,error:null};
 return new Harness({initialAlbumId:'album-a',isPlaybackAvailable:true,isLibraryUpdating:false,onlineArtworkEnabled:false,configuredLocalFolders:[],catalogRefreshRevision:0});
}
const sensitive='ACK [50@2] {play} /private/file.m4a https://private.invalid/?token=SECRET';
for(const lng of ['pt-BR','en']){
 const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(resolve(root,`src/locales/${lng}.json`),'utf8'))}}});
 test(`Local real component catches album and folder rejections with one translated private alert (${lng})`,async()=>{
  language=i18n;const h=setup();try{
   await h.settle();h.click(language.t('localBrowser.playAlbum'));await flush();assert.equal(requests.length,1);requests[0].reject(sensitive);await flush();
   let alerts=h.alerts();assert.equal(alerts.length,1);assert.ok(alerts[0].includes(language.t('player.requestError.failed')));for(const raw of ['ACK','SECRET','/private','https://'])assert.ok(!alerts[0].includes(raw));assert.equal(logs.length,1);
   await h.folders();assert.equal(h.alerts().length,0);assert.equal(h.folderClick(),undefined);await flush();assert.equal(requests.length,2);requests[1].reject(sensitive);await flush();
   alerts=h.alerts();assert.equal(alerts.length,1);assert.ok(alerts[0].includes(language.t('player.requestError.failed')));assert.equal(logs.length,2);assert.deepEqual(requests[1].args[1],0);
  }finally{h.close();}
 });
 test(`Local success and a new attempt clear previous feedback without duplicate commands (${lng})`,async()=>{
  language=i18n;const h=setup();try{
   await h.settle();h.click(language.t('localBrowser.playAlbum'));await flush();requests[0].reject(sensitive);await flush();assert.equal(h.alerts().length,1);
   h.click(language.t('localBrowser.playAlbum'));await flush();assert.equal(h.alerts().length,0);requests[1].resolve();await flush();assert.equal(h.alerts().length,0);
   await h.folders();h.folderClick();await flush();requests[2].resolve();await flush();assert.equal(h.alerts().length,0);assert.equal(requests.length,3);
  }finally{h.close();}
 });
 test(`Local ignores stale failures after newer success, context return, and unmount (${lng})`,async()=>{
  language=i18n;const h=setup();try{
   await h.settle();h.click(language.t('localBrowser.playAlbum'));await flush();h.click(language.t('localBrowser.playAlbum'));await flush();requests[1].resolve();await flush();requests[0].reject(sensitive);await flush();assert.equal(h.alerts().length,0);
   h.click(language.t('localBrowser.playAlbum'));await flush();const original=h.props;
   h.render({...original,subview:'metadata'});requests[2].reject(sensitive);await flush();h.render(original);assert.equal(h.alerts().length,0);
   h.click(language.t('localBrowser.playAlbum'));await flush();const pending=requests.at(-1);h.close();const writes=h.writes;pending.reject(sensitive);await flush();assert.equal(h.writes,writes);
  }finally{h.close();}
 });
 test(`Local folder feedback cannot reappear after folder navigation or availability change (${lng})`,async()=>{
  language=i18n;const h=setup();try{
   await h.settle();await h.folders();h.folderClick();await flush();requests[0].reject(sensitive);await flush();assert.equal(h.alerts().length,1);
   const directory=nodes(h.tree,n=>n.type==='div'&&n.props?.onClick&&n.props?.className?.includes('justify-between')&&markup(n).includes('Subfolder'))[0];assert.ok(directory);directory.props.onClick();await h.settle();assert.equal(h.alerts().length,0);
   h.click(language.t('localBrowser.upOneLevel'));await h.settle();h.click(language.t('localBrowser.upOneLevel'));await h.settle();assert.equal(h.alerts().length,0);
   h.folderClick();await flush();const original=h.props;h.render({...original,isPlaybackAvailable:false});requests[1].reject(sensitive);await flush();h.render(original);assert.equal(h.alerts().length,0);assert.equal(requests.length,2);
  }finally{h.close();}
 });
 test(`Local playback and catalog guards retain zero-IPC behavior (${lng})`,async()=>{
  language=i18n;const h=setup();try{
   await h.settle();h.render({...h.props,isPlaybackAvailable:false});h.click(language.t('localBrowser.playAlbum'));await flush();assert.equal(requests.length,0);assert.equal(h.alerts().length,0);
   await h.settle();h.render({...h.props,isPlaybackAvailable:true});await h.settle();const cover=deferred();coverRead=()=>cover.promise;
   h.click(language.t('localBrowser.playAlbum'));await flush();confirmedRevision++;snapshot={...snapshot,albums:[...snapshot.albums]};cover.resolve(null);await flush();assert.equal(requests.length,0);assert.equal(h.alerts().length,0);
  }finally{h.close();}
 });
}
