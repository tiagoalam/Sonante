import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as icons from "lucide-react";
import i18next from "i18next";
import { PlexDiscoverSession as DiscoverSession } from "../src/utils/plexDiscover.ts";
import * as presentation from "../src/utils/plexDiscoverPresentation.ts";
class PlexDiscoverSession extends DiscoverSession {
 constructor(load, children, recent=async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),random=()=>0) {
  super(load,children,recent,random);
 }
}
const i18n=i18next.createInstance();
await i18n.init({lng:"en",resources:Object.fromEntries(["en","pt-BR"].map(lang=>[lang,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${lang}.json`,import.meta.url),"utf8"))}]))});
let host=null;
const hooks={
  useEffect(){},
  useLayoutEffect(fn){if(host)host.effects.push(fn);},
  useRef(value){return host ? host.refs[host.cursor++]??={current:value} : {current:value};},
  useState(value){if(host?.values){const h=host,i=h.valueCursor++;if(!(i in h.values))h.values[i]=typeof value==="function"?value():value;return[h.values[i],next=>h.values[i]=typeof next==="function"?next(h.values[i]):next];}return [typeof value==="function"?value():value,()=>{}];},
  useSyncExternalStore(_,get){return get();}
};
const dependencies={react:{__esModule:true,...React,...hooks,default:React},"react/jsx-runtime":await import("react/jsx-runtime"),"react-i18next":{useTranslation:()=>({t:i18n.t.bind(i18n)})},"lucide-react":icons,
  "../utils/plexDiscover":{PlexDiscoverSession},"../utils/plexDiscoverPresentation":presentation,"./PlexImage":{PlexImage:({alt,...props})=>React.createElement("div",{"data-synthetic-artwork":true,"aria-label":alt,className:props.className})},
  "./AlbumArtworkPlaceholder":{AlbumArtworkPlaceholder:()=>React.createElement("div",{"data-synthetic-artwork":true,className:"h-full w-full bg-[#303030]"})}};
const choiceCompiled=ts.transpileModule(readFileSync(new URL('../src/components/MixChoiceCard.tsx',import.meta.url),'utf8'),{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const choiceExports={};new Function('require','exports',choiceCompiled)(name=>dependencies[name],choiceExports);
dependencies['./MixChoiceCard']=choiceExports;
const source=readFileSync(new URL("../src/components/PlexDiscoverView.tsx",import.meta.url),"utf8");
const compiled=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};new Function("require","exports",compiled)(name=>{assert.ok(name in dependencies,name);return dependencies[name];},exports);
const View=exports.PlexDiscoverView;
const flush=async()=>{for(let i=0;i<16;i++)await Promise.resolve();};
const scope={identity:{},serverId:"synthetic-server",libraryId:"7",language:"en",libraries:[{key:"7",title:"Synthetic music"}]};
const album={server_id:scope.serverId,library_id:"7",kind:"album",rating_key:"12",title:"Synthetic album",subtitle:"Synthetic artist",thumb:null,album:{rating_key:"12",title:"Synthetic album",artist:"Synthetic artist"}};
const artist={server_id:scope.serverId,library_id:"7",kind:"artist",rating_key:"13",title:"Synthetic artist",subtitle:"",thumb:null,album:null};
const hub={id:"server.recommended",title:"Synthetic server recommendation",items:[album,artist],partial:false};
const fixture=async(load=async()=>({language:scope.language,server_id:scope.serverId,library_id:scope.libraryId,hubs:[hub],partial:false}))=>{
  const session=new PlexDiscoverSession(load,async()=>({...hub,id:"artist.13",items:[album]}),async(server_id,library_id,language)=>({server_id,library_id,language,items:[{...album,added_at:100}],partial:false,dates_missing:false}));session.setContext(scope);session.ensure();await flush();
  return {session,scope,active:true,selection:"7",libraries:[{key:"7",title:"Synthetic music"}],librariesLoading:false,librariesError:false,onOpenAlbum(){},onOpenArtist(){},onLibrary(){},onLibraryChange(){},onRetryLibraries(){}};
};
const nodes=(tree,predicate,expand=false)=>Array.isArray(tree)?tree.flatMap(node=>nodes(node,predicate,expand)):!tree||typeof tree!=="object"?[]:[...(predicate(tree)?[tree]:[]),...nodes(expand && typeof tree.type==="function" ? tree.type(tree.props) : tree.props?.children,predicate,expand)];

test('App session wiring and actual Discover refresh request only hubs and recent albums in individual and All modes',async()=>{
 const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');
 const ast=ts.createSourceFile('App.tsx',app,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let constructor;
 const visit=node=>{if(ts.isNewExpression(node)&&node.expression.getText(ast)==='PlexDiscoverSession')constructor=node.getText(ast);ts.forEachChild(node,visit);};visit(ast);assert.ok(constructor);
 for(const selection of ['7','all']){
  const calls=[],libraries=[{key:'7',title:'Synthetic music'},{key:'8',title:'Other synthetic music'}],s={...scope,identity:{},libraryId:selection,libraries};
  const services={
   getDiscover:async(server_id,library_id,language)=>{calls.push(['hubs',library_id]);return{server_id,library_id,language,hubs:[{...hub,items:[{...album,library_id}]}],partial:false};},
   getDiscoverRecent:async(server_id,library_id,language)=>{calls.push(['recent',library_id]);return{server_id,library_id,language,items:[],partial:false,dates_missing:false};},
   getDiscoverChildren:async()=>assert.fail('opening Discover must not fetch children'),
   getDiscoverArtist:async()=>assert.fail('opening Discover must not fetch artist detail'),
   getLibraryMix:async()=>assert.fail('opening Discover does not automatically generate a mix'),
   getLibraryMixYears:async()=>assert.fail('opening Discover does not load decade choices'),
   getLibraryMixStyles:async()=>assert.fail('opening Discover does not load style choices')
  };
  const service=new Proxy(services,{get(target,key){assert.ok(key in target,`unexpected service: ${String(key)}`);return target[key];}});
  const session=new Function('PlexDiscoverSession','plexService',`return ${constructor};`)(DiscoverSession,service);
  session.setContext(s);session.ensure();await flush();
  const expected=selection==='all'?['7','8']:['7'];assert.deepEqual(calls.filter(c=>c[0]==='hubs').map(c=>c[1]),expected);assert.deepEqual(calls.filter(c=>c[0]==='recent').map(c=>c[1]),expected);
  const props={...await fixture(),session,scope:s,selection,libraries};
  const refresh=nodes(View(props),n=>n.type==='button'&&n.props['aria-label']===i18n.t('discover.refreshDiscover'))[0];assert.ok(refresh);refresh.props.onClick();await flush();
  assert.deepEqual(calls.filter(c=>c[0]==='hubs').map(c=>c[1]),[...expected,...expected]);assert.ok(calls.every(c=>['hubs','recent'].includes(c[0])));
  for(const language of ['en','pt-BR']){await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));assert.doesNotMatch(html,/Related discoveries|Descobertas relacionadas/);}
  await i18n.changeLanguage('en');
 }
 for(const path of ['../src/services/plex.ts','../src-tauri/src/lib.rs','../src-tauri/src/plex_discover.rs']){
  assert.doesNotMatch(readFileSync(new URL(path,import.meta.url),'utf8'),/getDiscoverContext|get_plex_discover_context|get_discover_context|\/status\/sessions\/history|\/similar\?/);
 }
});

test('actual mix card opens an internal bilingual preview and only explicit Play submits',async()=>{
 const calls=[],s={...scope,libraryId:'all',libraries:[{key:'7',title:'Music seven'},{key:'8',title:'Music eight'}]};
 const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),()=>0,undefined,async(server_id,library_id,language,limit)=>{
  calls.push(library_id);return{server_id,library_id,language,tracks:[{server_id,library_id,rating_key:'99',title:'Synthetic mix song',artist:'Synthetic mix artist',album_title:'Synthetic mix album',media_locator:{kind:'plex',server_id,rating_key:'99',part_key:'/library/parts/99/file.flac'}}],has_more:false,partial:false};
 });session.setContext(s);session.ensure();await flush();let plays=0;
 const props={...await fixture(),session,scope:s,selection:'all',libraries:s.libraries,isPlaybackAvailable:true,onPlayMix:async()=>{plays++;throw Error('private message');}};
 assert.equal(calls.length,0);let tree=View(props);const main=renderToStaticMarkup(React.createElement(View,props));assert.doesNotMatch(main,/Synthetic mix song|Generate another selection|Save as playlist|<table/);nodes(tree,n=>n.props?.['data-library-mix']!==undefined)[0].props.onClick();await flush();assert.equal(plays,0);assert.equal(calls.length,2);const selection=session.getSnapshot().mix.tracks;
 for(const [language,title] of [['en','Library mix'],['pt-BR','Mix da biblioteca']]){await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,new RegExp(title));assert.match(html,/Synthetic mix song/);assert.match(html,/Synthetic mix artist/);assert.match(html,/Synthetic mix album/);assert.match(html,/Music seven/);assert.match(html,/Music eight/);assert.doesNotMatch(html,/discover\.mix\.|private message/);assert.equal(session.getSnapshot().mix.tracks,selection);}
 await i18n.changeLanguage('en');tree=View(props);nodes(tree,n=>n.type==='button'&&n.props.children==='Play')[0].props.onClick();await flush();assert.equal(plays,1);assert.match(renderToStaticMarkup(React.createElement(View,props)),/mix could not be played/);
 props.active=false;tree=View(props);assert.equal(nodes(tree,n=>n.type==='button'&&n.props.children==='Play')[0].props.disabled,true);
 const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');assert.match(app,/await appPlayback\.run\(intent => audioService\.playTracks\(mixPlaybackMetadata\(tracks\), 0, intent\)\)/);
});
test('actual total mix failure is neutral, has no partial preview and keeps Play disabled',async()=>{
 const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),()=>0,undefined,async()=>{throw 'PLEX_DISCOVER_INVALID';});
 session.setContext(scope);session.ensure();await flush();session.openMix();await flush();
 const props={...await fixture(),session,isPlaybackAvailable:true,onPlayMix:async()=>assert.fail('no playable tracks')};
 for(const [language,text] of [['en','No tracks were confirmed'],['pt-BR','Nenhuma faixa foi confirmada']]){
  await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,new RegExp(text));assert.doesNotMatch(html,/This preview is partial|Esta prévia é parcial|when Plex is available|quando o Plex estiver disponível/);
  const play=nodes(View(props),n=>n.type==='button'&&n.props.children===i18n.t('discover.mix.play'))[0];assert.equal(play.props.disabled,true);
 }
 await i18n.changeLanguage('en');
});

test('actual artist suggestions replace only the artist family with bilingual cards and preserve artist navigation/focus',async()=>{
 const raw=JSON.parse(readFileSync(new URL('./fixtures/plex-discover-artist-recommendations.json',import.meta.url),'utf8')),calls=[];
 const session=new PlexDiscoverSession(async()=>({server_id:scope.serverId,library_id:'7',language:'en',partial:false,hubs:raw.MediaContainer.Hub.map((h,index)=>({id:h.hubIdentifier,context:h.context,title:h.title,partial:false,artist_recommendation:index===0,
  items:h.Metadata.map(i=>({...album,rating_key:i.ratingKey,album:{...album.album,rating_key:i.ratingKey,artist_rating_key:i.parentRatingKey}})),
  artist_candidates:h.Metadata.map(i=>({...artist,rating_key:i.parentRatingKey,title:i.parentTitle,thumb:{server_id:scope.serverId,path:i.parentThumb}}))
 }))}),async(server,id,kind,key)=>{calls.push({server,id,kind,key});return{...hub,items:[album]};});
 session.setContext(scope);session.ensure();await flush();const props={...await fixture(),session,onOpenAlbum:()=>assert.fail('artist suggestion must only navigate'),onOpenArtist:artist=>calls.push(artist)};
 for(const [language,title] of [['en','Artist suggestions'],['pt-BR','Sugestões de artistas']]){
  await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,new RegExp(`<section aria-label="${title}"`));
  assert.doesNotMatch(html,/Synthetic server row artist/);assert.match(html,/Synthetic server row label/);assert.match(html,/Synthetic server row genre/);assert.match(html,/explore-album-shelf/);
 }
 await i18n.changeLanguage('en');const selection=session.getSnapshot().hubs[0];assert.equal(selection.items.length,3);
 assert.equal(selection.items[0].thumb.path,'/library/metadata/501/thumb/1');assert.ok(selection.items.every(i=>i.kind==='artist'&&!i.album));
 const selected=selection.items[2],focus=[],target={dataset:{discoverItem:selected.presentation_key ?? presentation.discoverItemKey(selected)},focus:o=>focus.push(['card',o])};
 const state={refs:[],cursor:0,effects:[]},render=()=>{state.cursor=0;state.effects=[];host=state;const tree=View(props);host=null;return tree;};
 let tree=render();const node={scrollTop:0,querySelector:()=>({focus:o=>focus.push(['heading',o])}),querySelectorAll:()=>[target]};state.refs[0].current=node;state.effects.forEach(fn=>fn());
 nodes(tree,n=>n.props?.onScroll)[0].props.onScroll({currentTarget:{scrollTop:700}});session.saveShelfPosition(selection.id,150);
 nodes(tree,n=>n.props?.['data-discover-item']===(selected.presentation_key ?? presentation.discoverItemKey(selected)),true)[0].props.onClick({currentTarget:target});await flush();
 assert.equal(calls.length,1);assert.equal(calls[0].server_id,scope.serverId);assert.equal(calls[0].library_id,'7');assert.equal(calls[0].rating_key,'503');assert.equal(session.getSnapshot().detail,null);props.active=false;tree=render();state.effects.forEach(fn=>fn());props.active=true;tree=render();state.effects.forEach(fn=>fn());
 assert.equal(node.scrollTop,700);assert.equal(session.shelfPosition(selection.id),150);assert.equal(session.getSnapshot().hubs[0].items[2],selected);assert.deepEqual(focus,[['card',{preventScroll:true}]]);
});

test('actual aggregated played-artist shelf has its own bilingual title, source labels and source-safe focus return',async()=>{
 const raw=JSON.parse(readFileSync(new URL('./fixtures/plex-discover-recent-played-artists.json',import.meta.url),'utf8'));
 const libraries=[{key:'1',title:'Synthetic library one'},{key:'2',title:'Synthetic library two'}],all={...scope,libraryId:'all',libraries},calls=[];
 const session=new PlexDiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,partial:false,hubs:raw.MediaContainer.Hub.map((h,index)=>({id:h.hubIdentifier,context:h.context,title:h.title,partial:false,recent_family:index===0?'played_artists':null,items:h.Metadata.map(i=>({...artist,server_id,library_id,rating_key:i.ratingKey,title:i.title,last_viewed_at:i.lastViewedAt+(library_id==='2'?100:0)}))}))}),async(server,id,kind,key)=>{calls.push({server,id,kind,key});return{id:'children',title:'Synthetic albums',partial:false,items:[{...album,library_id:id}]};});
 session.setContext(all);session.ensure();await flush();
 const props={session,scope:all,active:true,selection:'all',libraries,librariesLoading:false,librariesError:false,onOpenAlbum:()=>assert.fail('artist navigation must not open/play album'),onOpenArtist:artist=>calls.push(artist),onLibrary(){},onLibraryChange(){},onRetryLibraries(){}};
 for(const [language,title] of [['en','Recently played artists'],['pt-BR','Artistas reproduzidos recentemente']]){
  await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));
  assert.equal((html.match(new RegExp(`<section aria-label="${title}"`,'g'))??[]).length,1);
  assert.match(html,/Synthetic library one/);assert.match(html,/Synthetic library two/);assert.match(html,/explore-album-shelf/);assert.doesNotMatch(html,/discover\.standard\.recentPlayedArtists/);
 }
 await i18n.changeLanguage('en');
 const selected=session.getSnapshot().hubs.find(h=>h.recent_family==='played_artists').items.find(i=>i.library_id==='2'),focus=[];
 const target={dataset:{discoverItem:selected.presentation_key ?? presentation.discoverItemKey(selected)},focus:options=>focus.push(['card',options])};
 const state={refs:[],cursor:0,effects:[]},render=()=>{state.cursor=0;state.effects=[];host=state;const tree=View(props);host=null;return tree;};
 let tree=render();const node={scrollTop:0,querySelector:()=>({focus:options=>focus.push(['heading',options])}),querySelectorAll:()=>[target]};state.refs[0].current=node;state.effects.forEach(fn=>fn());
 nodes(tree,n=>n.props?.onScroll)[0].props.onScroll({currentTarget:{scrollTop:650}});
 const shelf=session.getSnapshot().hubs.find(h=>h.recent_family==='played_artists');session.saveShelfPosition(shelf.id,320);
 nodes(tree,n=>n.props?.['data-discover-item']===(selected.presentation_key ?? presentation.discoverItemKey(selected)),true)[0].props.onClick({currentTarget:target});await flush();
 assert.equal(calls.length,1);assert.equal(calls[0].server_id,scope.serverId);assert.equal(calls[0].library_id,'2');assert.equal(calls[0].rating_key,selected.rating_key);assert.equal(session.getSnapshot().detail,null);props.active=false;render();props.active=true;tree=render();state.effects.forEach(fn=>fn());
 assert.equal(node.scrollTop,650);assert.equal(session.shelfPosition(shelf.id),320);assert.deepEqual(focus,[['card',{preventScroll:true}]]);
});

test("actual Discover component preserves server sections, accessible navigation and translated states",async()=>{
  const props=await fixture();
  const html=renderToStaticMarkup(React.createElement(View,props));
  assert.match(html,/Synthetic server recommendation/);assert.doesNotMatch(html,/Related discoveries/);assert.match(html,/data-plex-discover/);assert.match(html,/focus-visible:ring/);assert.match(html,/overflow-y-auto/);
  assert.match(html,/aria-label="Open Synthetic album"/);assert.doesNotMatch(html,/Because you listened|Play radio/);
  const auth=await fixture(async()=>{throw "PLEX_DISCOVER_AUTH";});const error=renderToStaticMarkup(React.createElement(View,auth));
  assert.match(error,/role="alert"/);assert.match(error,/Refresh/);assert.doesNotMatch(error,/No recommendations/);
  await i18n.changeLanguage("pt-BR");const pt=renderToStaticMarkup(React.createElement(View,props));assert.doesNotMatch(pt,/discover\.[a-zA-Z]/);await i18n.changeLanguage("en");
  if(process.env.SONANTE_SYNTHETIC_PREVIEW)writeFileSync(process.env.SONANTE_SYNTHETIC_PREVIEW,html);
});

test("actual card actions navigate without playback and preserve main/detail position and focus on return",async()=>{
  const props=await fixture();const opened=[],openedArtists=[];props.onOpenAlbum=value=>opened.push(value);props.onOpenArtist=value=>openedArtists.push(value);
  const state={refs:[],cursor:0,effects:[]};
  const render=()=>{state.cursor=0;state.effects=[];host=state;const tree=View(props);host=null;return tree;};
  let tree=render();const focus=[];
  const target={dataset:{discoverItem:presentation.discoverItemKey(artist)},focus:options=>focus.push(["card",options])};
  const albumTarget={isConnected:true,focus:options=>focus.push(["album",options])};
  const node={scrollTop:0,querySelector:()=>({focus:options=>focus.push(["heading",options])}),querySelectorAll:()=>[target]};state.refs[0].current=node;state.effects.forEach(fn=>fn());
  nodes(tree,node=>node.props?.onScroll)[0].props.onScroll({currentTarget:{scrollTop:540}});assert.equal(props.session.position,540);
  let cards=nodes(tree,node=>node.props?.["data-discover-item"],true);
  cards.find(card=>card.props["data-discover-item"]===presentation.discoverItemKey(album)).props.onClick({currentTarget:albumTarget});
  assert.deepEqual(opened,[{...album.album,server_id:scope.serverId,library_id:scope.libraryId,library_title:"Synthetic music"}]);
  props.active=false;render();state.effects.forEach(fn=>fn());props.active=true;tree=render();state.effects.forEach(fn=>fn());assert.equal(node.scrollTop,540);
  cards=nodes(tree,node=>node.props?.["data-discover-item"],true);cards.find(card=>card.props["data-discover-item"]===presentation.discoverItemKey(artist)).props.onClick({currentTarget:target});await flush();
  assert.equal(openedArtists[0].rating_key,artist.rating_key);assert.equal(openedArtists[0].server_id,scope.serverId);assert.equal(props.session.getSnapshot().detail,null);props.active=false;render();props.active=true;tree=render();state.effects.forEach(fn=>fn());assert.equal(node.scrollTop,540);assert.deepEqual(focus,[["album",{preventScroll:true}],["card",{preventScroll:true}]]);
  props.active=false;tree=render();cards=nodes(tree,node=>node.props?.["data-discover-item"],true);const before=opened.length;cards[0].props.onClick({currentTarget:albumTarget});assert.equal(opened.length,before);
});

test("library lookup failure offers an explicit retry and no-scope navigation cannot expose stale rows",async()=>{
  const props=await fixture();let retried=0;props.scope=null;props.librariesError=true;props.onRetryLibraries=()=>retried++;
  const tree=View(props);const buttons=nodes(tree,node=>node.type==="button");buttons.find(button=>Array.isArray(button.props.children)).props.onClick();assert.equal(retried,1);
  const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/role="alert"/);assert.doesNotMatch(html,/Synthetic server recommendation/);
});

test("App keeps Discover keyboard navigation ahead of playback shortcuts and guards full-catalog loading",()=>{
  const app=readFileSync(new URL("../src/App.tsx",import.meta.url),"utf8");
  const guard=app.indexOf('activeEl.closest("[data-plex-discover]")');const playback=app.indexOf('audioService.togglePlay()',guard);assert.ok(guard>0 && playback>guard);
  assert.match(app,/if \(centralSection !== "plex" \|\| plexArea !== "library"\) return;/);
  assert.match(app,/page\.identity === discoverIdentity/);
  assert.match(app,/onLibraryChange=\{setDiscoverSelection\}/);
  assert.doesNotMatch(source,/audioService|playTracks|setQueue|setInterval/);
});

test("actual All component has one shelf per proven family, sources on cards and no library pagination",async()=>{
 const libs=Array.from({length:7},(_,i)=>({key:String(i+1),title:`Biblioteca ${i+1} — Unicode 日本 🎶 nome muito longo` }));
 const s={...scope,identity:{},libraryId:"all",libraries:libs};
 const session=new PlexDiscoverSession(async(server,id,language)=>({language,server_id:server,library_id:id,partial:false,hubs:['added','played'].map(recent_family=>({id:`opaque-${recent_family}`,recent_family,title:`Server title`,partial:false,items:[{...album,library_id:id,added_at:100+Number(id),last_viewed_at:null,title:"Long album with Unicode 日本 🎶 and more text"}]}))}),async()=>assert.fail());
 session.setContext(s);session.ensure();for(let i=0;i<10;i++)await flush();
 const props={...await fixture(),session,scope:s,selection:"all",libraries:libs};
 const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/All libraries/);assert.doesNotMatch(html,/Next libraries|Previous libraries|Libraries 1–4/);assert.match(html,/data-synthetic-artwork/);assert.match(html,/explore-album-shelf flex gap-3 overflow-x-auto/);assert.match(html,/Items without a date follow/);
 const tree=View(props),shelves=nodes(tree,node=>typeof node.type==='function'&&node.type.name==='Shelf');assert.equal(shelves.length,2);assert.deepEqual(shelves.map(node=>node.props.hub.recent_family),['added','played']);assert.ok(shelves.every(node=>node.props.showOrigin));
 const cards=nodes(tree,node=>node.props?.['data-discover-item'],true);assert.ok(cards.every(node=>nodes(node,n=>n.type==='span'&&n.props.title?.startsWith('Biblioteca')).length===1),'source is visible on each card, not only in a shared heading');
 await i18n.changeLanguage('pt-BR');const pt=renderToStaticMarkup(React.createElement(View,props));assert.match(pt,/Adicionados recentemente/);assert.match(pt,/Ouvidos recentemente/);assert.doesNotMatch(pt,/discover\.[a-zA-Z]/);await i18n.changeLanguage('en');
 const css=readFileSync(new URL('../src/index.css',import.meta.url),'utf8');assert.match(css,/\.explore-album-shelf\s*\{\s*scrollbar-width: none;/);
});
test("no accessible music libraries is an empty state rather than a missing-server instruction",async()=>{
 const props={...await fixture(),scope:null,selection:"all",libraries:[]};const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/No music libraries are available/);assert.doesNotMatch(html,/Choose a Plex server|role="alert"/);
});
test("Discover album details also shield navigation Space from the global playback shortcut",async()=>{
 const app=readFileSync(new URL("../src/App.tsx",import.meta.url),"utf8");assert.match(app,/plexArea === "discover" \? \(\s*<div data-plex-discover className="contents">/);
 const start=app.indexOf('const handleKeyDown = (e: KeyboardEvent) => {'),end=app.indexOf('window.addEventListener("keydown", handleKeyDown)',start);assert.ok(start>0 && end>start);
 const handler=ts.transpileModule(app.slice(start,end),{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.CommonJS}}).outputText;
 class ElementFixture {tagName="BUTTON";constructor(protectedNavigation){this.protectedNavigation=protectedNavigation;}closest(selector){if(selector==="[data-now-playing-queue-content]")return null;assert.equal(selector,"[data-plex-discover]");return this.protectedNavigation?{}:null;}}
 let toggles=0,prevented=0;const document={activeElement:new ElementFixture(true)};
 const run=new Function("nowPlayingHosting","document","HTMLElement","playbackAvailableRef","statusRef","audioService",'const showSettings=false,showAbout=false;'+handler+'return handleKeyDown;')({getSnapshot:()=>({mode:'closed'})},document,ElementFixture,{current:true},{current:{}},{togglePlay:()=>{toggles++;return Promise.resolve();}});
 const space={key:" ",code:"Space",ctrlKey:false,metaKey:false,preventDefault(){prevented++;}};run(space);assert.equal(toggles,0);assert.equal(prevented,0,"native navigation activation stays available");
 document.activeElement=new ElementFixture(false);run(space);assert.equal(toggles,1,"the existing shortcut remains outside Discover");assert.equal(prevented,1);
});

test("actual hero action renews only the scoped recent selection and never draws in render",async()=>{
 const props=await fixture();assert.doesNotMatch(renderToStaticMarkup(React.createElement(View,props)),/The only recent album available|Chosen from up to|A selection to explore|aria-live="polite"/);let draws=0;
 const pool=[{...album,added_at:200,title:'Synthetic newest',rating_key:'20',album:{...album.album,rating_key:'20'}},{...album,added_at:100,title:'Synthetic second',rating_key:'21',album:{...album.album,rating_key:'21'}}];
 const session=new PlexDiscoverSession(async()=>({language:'en',server_id:scope.serverId,library_id:'7',hubs:[hub],partial:false}),async()=>assert.fail(),async()=>({language:'en',server_id:scope.serverId,library_id:'7',items:pool,partial:false,dates_missing:false}),()=>{draws++;return 0;});
 session.setContext(scope);session.ensure();await flush();props.session=session;
 const shelves=session.getSnapshot().libraries[0].page.hubs;let tree=View(props),html=renderToStaticMarkup(React.createElement(View,props));assert.doesNotMatch(html,/Chosen from up to|The only recent album available|A selection to explore/);assert.match(html,/Another discovery/);assert.doesNotMatch(html,/aria-live="polite"/);assert.equal(draws,1);
 const button=nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes('Another discovery'))[0];assert.ok(button);button.props.onClick();assert.equal(session.getSnapshot().hero.item.rating_key,'21');assert.equal(draws,2);assert.equal(session.getSnapshot().libraries[0].page.hubs,shelves);
 for(let i=0;i<3;i++)renderToStaticMarkup(React.createElement(View,props));assert.equal(draws,2,'SSR/re-render does not select again');
 const opened=[];props.onOpenAlbum=value=>opened.push(value);tree=View(props);const card=nodes(tree,n=>n.props?.['data-discover-item']===presentation.discoverItemKey(session.getSnapshot().hero.item),true)[0];card.props.onClick({currentTarget:{focus(){},isConnected:true}});assert.equal(opened.length,1);assert.equal(opened[0].rating_key,'21');assert.equal(opened[0].library_id,'7');assert.equal(opened[0].server_id,scope.serverId);assert.equal(draws,2);
 await i18n.changeLanguage('pt-BR');html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/Outra descoberta/);assert.doesNotMatch(html,/Sorteado entre até|Único álbum recente disponível|Uma seleção para descobrir/);assert.doesNotMatch(html,/discover\.[a-zA-Z]/);await i18n.changeLanguage('en');
 const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');assert.match(app,/plexService\.getDiscoverChildren, plexService\.getDiscoverRecent/);
});

test("actual hero shows a discreet outside-shelf empty state rather than duplicating a recently added album",async()=>{
 const props=await fixture(),recentAlbum={...album,added_at:100};
 const session=new PlexDiscoverSession(async()=>({language:'en',server_id:scope.serverId,library_id:'7',hubs:[{...hub,recent_family:'added',items:[recentAlbum]}],partial:false}),async()=>assert.fail(),async()=>({language:'en',server_id:scope.serverId,library_id:'7',items:[recentAlbum],partial:false,dates_missing:false}));
 session.setContext(scope);session.ensure();await flush();props.session=session;assert.equal(session.getSnapshot().hero.item,null);
 const tree=View(props),cards=nodes(tree,n=>n.props?.['data-discover-item'],true);assert.equal(cards.length,1,'only the shelf card exists');const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/No recent albums are available outside/);assert.doesNotMatch(html,/Another discovery/);
 await i18n.changeLanguage('pt-BR');assert.match(renderToStaticMarkup(React.createElement(View,props)),/fora da prateleira de adicionados recentemente/);await i18n.changeLanguage('en');
});

test('internal mix navigation restores main/card and mix/action focus with independent scroll positions',async()=>{
 let queries=0,plays=0;const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),undefined,()=>0,undefined,async(server_id,library_id,language)=>{queries++;return{server_id,library_id,language,tracks:[{server_id,library_id,rating_key:'9',title:'Song',media_locator:{kind:'plex',server_id,rating_key:'9',part_key:'/library/parts/9/file'}}],has_more:false,partial:false};});
 session.setContext(scope);session.ensure();await flush();const props={...await fixture(),session,onPlayMix:async()=>plays++,onSaveMix:async()=>assert.fail(),isPlaybackAvailable:true};
 const h={refs:[],cursor:0,effects:[]},focus=[];const target=name=>({focus:options=>focus.push([name,options])});const node={scrollTop:0,querySelector:selector=>target(selector),querySelectorAll:()=>[]};
 const render=()=>{h.cursor=0;h.effects=[];host=h;const tree=View(props);host=null;h.refs[0].current=node;h.effects.forEach(fn=>fn());return tree;};
 let tree=render();nodes(tree,n=>n.props?.onScroll)[0].props.onScroll({currentTarget:{scrollTop:420}});nodes(tree,n=>n.props?.['data-library-mix']!==undefined)[0].props.onClick();await flush();tree=render();assert.equal(node.scrollTop,0);assert.equal(focus.at(-1)[0],'h1');const selection=session.getSnapshot().mix.tracks;
 nodes(tree,n=>n.props?.onScroll)[0].props.onScroll({currentTarget:{scrollTop:780}});nodes(tree,n=>n.props?.['data-mix-focus']==='save')[0].props.onFocus();nodes(tree,n=>n.type==='button'&&n.props.children?.at?.(-1)===i18n.t('discover.back'))[0].props.onClick();tree=render();assert.equal(node.scrollTop,420);assert.equal(focus.at(-1)[0],'[data-library-mix]');
 nodes(tree,n=>n.props?.['data-library-mix']!==undefined)[0].props.onClick();tree=render();assert.equal(node.scrollTop,780);assert.equal(focus.at(-1)[0],'[data-mix-focus="save"]:not(:disabled)');assert.equal(session.getSnapshot().mix.tracks,selection);assert.equal(queries,1);assert.equal(plays,0);
});
test('name-only save dialog traps keyboard focus, cancels safely and displays validation/write errors bilingually',async()=>{
 const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),undefined,()=>0,undefined,async(server_id,library_id,language)=>({server_id,library_id,language,tracks:[{server_id,library_id,rating_key:'9',title:'Song',media_locator:{kind:'plex',server_id,rating_key:'9',part_key:'/library/parts/9/file'}}],has_more:false,partial:false}));session.setContext(scope);session.openMix();await flush();let writes=0,plays=0;
 const props={...await fixture(),session,onPlayMix:async()=>plays++,onSaveMix:async()=>{writes++;throw 'playlists.duplicateName';},isPlaybackAvailable:true};
 const h={refs:[],cursor:0,effects:[]},focus=[];const render=()=>{h.cursor=0;h.effects=[];host=h;const tree=View(props);host=null;h.refs[0].current={scrollTop:0,querySelector:()=>({focus(){}})};h.refs[4].current={focus:()=>focus.push('save')};h.refs[5].current={focus:()=>focus.push('name')};h.effects.forEach(fn=>fn());return tree;};
 let tree=render();nodes(tree,n=>n.props?.['data-mix-focus']==='save')[0].props.onClick();tree=render();assert.equal(focus.at(-1),'name');assert.equal(nodes(tree,n=>n.type==='input').length,1);let form=nodes(tree,n=>n.type==='form')[0];form.props.onSubmit({preventDefault(){}});await flush();assert.equal(writes,0);assert.match(renderToStaticMarkup(React.createElement(View,props)),new RegExp(i18n.t('playlists.nameRequired')));
 tree=render();nodes(tree,n=>n.type==='input')[0].props.onChange({target:{value:'Synthetic'}});tree=render();nodes(tree,n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();assert.equal(writes,1);
 for(const language of ['en','pt-BR']){await i18n.changeLanguage(language);assert.match(renderToStaticMarkup(React.createElement(View,props)),new RegExp(i18n.t('playlists.duplicateName')));}await i18n.changeLanguage('en');
 tree=render();const dialog=nodes(tree,n=>n.props?.role==='dialog')[0],first={focus:()=>focus.push('first')},last={focus:()=>focus.push('last')};let prevented=0;
 dialog.props.onKeyDown({key:'Tab',shiftKey:false,preventDefault:()=>prevented++,currentTarget:{querySelectorAll:()=>[first,last],ownerDocument:{activeElement:last}}});assert.equal(focus.at(-1),'first');
 dialog.props.onKeyDown({key:'Tab',shiftKey:true,preventDefault:()=>prevented++,currentTarget:{querySelectorAll:()=>[first,last],ownerDocument:{activeElement:first}}});assert.equal(focus.at(-1),'last');assert.equal(prevented,2);
 dialog.props.onKeyDown({key:'Tab',preventDefault:()=>prevented++,currentTarget:{querySelectorAll:()=>[]}});assert.equal(prevented,3);
 dialog.props.onKeyDown({key:'Escape',preventDefault(){},stopPropagation(){}});tree=render();assert.equal(nodes(tree,n=>n.props?.role==='dialog').length,0);assert.equal(focus.at(-1),'save');assert.equal(plays,0);assert.equal(writes,1);
});
test('actual App save callback uses existing service and refreshes Playlists only after successful persistence',async()=>{
 const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8'),ast=ts.createSourceFile('App.tsx',app,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let callback;
 const visit=node=>{if(ts.isJsxAttribute(node)&&node.name.getText(ast)==='onSaveMix')callback=node.initializer.expression.getText(ast);ts.forEachChild(node,visit);};visit(ast);assert.ok(callback);
 const body=ts.transpileModule(`const callback=${callback};`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;let revision=0,calls=[],resolve;const pending=new Promise(r=>resolve=r);
 const make=createWithItems=>new Function('playlistsService','isDuplicatePlaylistNameError','setPlaylistRevision',`${body};return callback;`)({createWithItems},cause=>String(cause).includes('playlist_name_duplicate'),update=>revision=update(revision));
 const items=[{media_locator:{kind:'plex',server_id:'s',rating_key:'8',part_key:'/library/parts/8/file'},metadata:{title:'Synthetic'}}];const save=make((name,items)=>{calls.push({name,items});return pending;});const result=save('Synthetic',items);assert.equal(revision,0);assert.equal(calls[0].items,items);resolve({id:'new'});await result;assert.equal(revision,1);
 for(const [cause,expected] of [['playlist_name_duplicate','playlists.duplicateName'],[Error('private'),'playlists.createAndAddError']])await assert.rejects(make(async()=>{throw cause;})('Synthetic',items),error=>error===expected);
 assert.equal(revision,1);assert.match(app,/<PlaylistsView[^>]*refreshKey=\{playlistRevision\}/);assert.doesNotMatch(callback,/audioService|playTracks|\.play\(/);
 const playlists=readFileSync(new URL('../src/components/PlaylistsView.tsx',import.meta.url),'utf8');assert.match(playlists,/\}, \[t, refreshKey\]\)/);
 // Execute the actual existing list effect against disposable service data.
 const file=ts.createSourceFile('PlaylistsView.tsx',playlists,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let effect;
 const find=node=>{if(ts.isCallExpression(node)&&node.expression.getText(file)==='useEffect'&&node.arguments[1]?.getText(file)==='[t, refreshKey]')effect=node.arguments[0].getText(file);ts.forEachChild(node,find);};find(file);assert.ok(effect);
 const effectBody=ts.transpileModule(`const effect=${effect};`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;let listed=[],stored=[];
 const run=new Function('playlistsService','setPlaylists','setError','setLoading','logError','t',`${effectBody};return effect;`)({list:async()=>structuredClone(stored)},items=>listed=items,()=>assert.fail(),()=>{},()=>assert.fail(),key=>key);
 const cleanup=run();await flush();assert.deepEqual(listed,[]);cleanup();stored=[{id:'new',name:'Synthetic',items}];run();await flush();assert.deepEqual(listed,stored);
});

test('actual decade card offers explicit bilingual album-year choices and reuses preview, change and save actions',async()=>{
 const calls=[],session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),undefined,()=>0,undefined,async(server_id,library_id,language,limit,decade)=>{calls.push({limit,decade});return{server_id,library_id,language,tracks:[{server_id,library_id,rating_key:'9',title:'Synthetic decade song',media_locator:{kind:'plex',server_id,rating_key:'9',part_key:'/library/parts/9/file'}}],has_more:false,partial:false};},async(server_id,library_id,language)=>({server_id,library_id,language,years:[1989,1990,1999,2000],partial:true}));session.setContext(scope);session.ensure();await flush();let plays=0,saved;
 const props={...await fixture(),session,isPlaybackAvailable:true,onPlayMix:async()=>plays++,onSaveMix:async(name,items)=>saved={name,items}};
 nodes(View(props),n=>n.props?.['data-decade-mix']!==undefined)[0].props.onClick();await flush();assert.equal(calls.length,0);let tree=View(props);assert.deepEqual(nodes(tree,n=>n.props?.['data-decade']!==undefined).map(n=>n.props['data-decade']),[2000,1990,1980]);
 for(const language of ['en','pt-BR']){await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,new RegExp(i18n.t('discover.decade.title')));assert.match(html,new RegExp(i18n.t('discover.decade.partial')));}await i18n.changeLanguage('en');
 nodes(View(props),n=>n.props?.['data-decade']===1990)[0].props.onClick();await flush();assert.equal(calls[0].decade,1990);assert.equal(calls[0].limit,100);assert.match(renderToStaticMarkup(React.createElement(View,props)),/Mix · 1990s/);assert.equal(plays,0);
 tree=View(props);nodes(tree,n=>n.props?.['data-mix-focus']==='save')[0].props.onClick();session.setMixSaveName('Synthetic decade');nodes(View(props),n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();assert.equal(saved.items[0].media_locator.rating_key,'9');assert.equal(plays,0);
 nodes(View(props),n=>n.type==='button'&&n.props.children===i18n.t('discover.decade.change'))[0].props.onClick();assert.equal(session.getSnapshot().decades.opened,true);nodes(View(props),n=>n.props?.['data-decade']===1990)[0].props.onClick();assert.equal(calls.length,1);
});

test('shared colored selector cards retain full labels, decade intervals, stable colors and readable contrast',async()=>{
 const label='A very long musical style label with words and distinctions that must remain fully readable';
 const card=React.createElement(choiceExports.MixChoiceCard,{identity:'style:test',kind:'style',label,disabled:false,onClick(){}});
 const html=renderToStaticMarkup(card);assert.ok(html.includes(label));assert.doesNotMatch(html,/truncate|line-clamp/);assert.match(html,/focus-visible:ring/);assert.equal(choiceExports.mixChoiceColor('style:test'),choiceExports.mixChoiceColor('style:test'));
 const colors=new Set(Array.from({length:12},(_,i)=>choiceExports.mixChoiceColor('decade:'+(1900+i*10))));assert.ok(colors.size>=3);
 for(const color of colors){const rgb=color.slice(1).match(/../g).map(hex=>parseInt(hex,16)/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4);const lum=.2126*rgb[0]+.7152*rgb[1]+.0722*rgb[2];assert.ok(1.05/(lum+.05)>=4.5);}
 const interval=renderToStaticMarkup(React.createElement(choiceExports.MixChoiceCard,{identity:'decade:1990',kind:'decade',label:'Anos 1990',subtitle:'1990–1999',disabled:false,onClick(){}}));assert.match(interval,/Anos 1990/);assert.match(interval,/1990–1999/);
});
test('actual style card opens a bilingual selector and reuses save/change without playback',async()=>{
 const calls=[],label='A complete long style label for synthetic music';let plays=0,saved;
 const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),undefined,()=>0,undefined,async(server_id,library_id,language,limit,decade,style)=>{calls.push({limit,decade,style});return{server_id,library_id,language,tracks:[{server_id,library_id,rating_key:'9',title:'Synthetic style song',media_locator:{kind:'plex',server_id,rating_key:'9',part_key:'/library/parts/9/file'}}],has_more:false,partial:false};},undefined,async(server_id,library_id,language)=>({server_id,library_id,language,field:'artist.style',choices:[{id:'88',title:label}],partial:true}));session.setContext(scope);session.ensure();await flush();const props={...await fixture(),session,isPlaybackAvailable:true,onPlayMix:async()=>plays++,onSaveMix:async(name,items)=>saved={name,items}};
 nodes(View(props),n=>n.props?.['data-style-mix']!==undefined)[0].props.onClick();await flush();assert.equal(calls.length,0);const key=session.styleOptions()[0].key;
 for(const language of ['en','pt-BR']){await i18n.changeLanguage(language);const html=renderToStaticMarkup(React.createElement(View,props));assert.ok(html.includes(label));assert.match(html,new RegExp(i18n.t('discover.style.partial')));}await i18n.changeLanguage('en');
 nodes(View(props),n=>n.props?.['data-style']===key)[0].props.onClick();await flush();assert.deepEqual(calls[0].style,{field:'artist.style',ids:['88']});assert.equal(calls[0].decade,undefined);assert.equal(plays,0);nodes(View(props),n=>n.props?.['data-mix-focus']==='save')[0].props.onClick();session.setMixSaveName('Synthetic');nodes(View(props),n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();assert.equal(saved.items[0].media_locator.rating_key,'9');assert.equal(plays,0);
 nodes(View(props),n=>n.type==='button'&&n.props.children===i18n.t('discover.style.change'))[0].props.onClick();assert.equal(session.getSnapshot().styles.opened,true);nodes(View(props),n=>n.props?.['data-style']===key)[0].props.onClick();assert.equal(calls.length,1);
});

test('actual Discover album play buttons have translated titles and accessible names without key fallback',async()=>{
 const props={...await fixture(),isPlaybackAvailable:true,onPlayAlbum:async()=>assert.fail('render must not play')};
 try {
  for(const [language,label] of [['en','Play Album'],['pt-BR','Tocar Álbum']]){
   await i18n.changeLanguage(language);
   assert.equal(i18n.exists('common.playAlbum'),true);assert.equal(i18n.t('common.playAlbum'),label);
   const buttons=nodes(View(props),n=>n.type==='button'&&n.props.title===label,true);
   assert.ok(buttons.length>0,'actual play buttons must be present');
   for(const button of buttons){assert.equal(button.props['aria-label'],label);assert.equal(button.props.title,label);}
   const html=renderToStaticMarkup(React.createElement(View,props));
   assert.doesNotMatch(html,/albumView\.playAlbum|common\.playAlbum/);
  }
 } finally {await i18n.changeLanguage('en');}
});

test('actual album mask is a sibling button, never opens the album, blocks double submission and invalidates late replies',async()=>{
 const props=await fixture();let opens=0,plays=0,resolve;
 props.onOpenAlbum=()=>opens++;props.isPlaybackAvailable=true;
 props.onPlayAlbum=async(album,current)=>{assert.equal(album.library_id,'7');assert.equal(album.server_id,scope.serverId);await new Promise(r=>resolve=r);if(current())plays++;};
 const state={refs:[],cursor:0,effects:[]},render=()=>{state.cursor=0;state.effects=[];host=state;const tree=View(props);host=null;return tree;};
 const tree=render(),expanded=nodes(tree,n=>n.type==='button',true),play=expanded.find(n=>n.props['aria-label']===i18n.t('common.playAlbum'));
 assert.ok(play);assert.equal(play.props.type,'button');assert.equal(play.props.disabled,false);
 const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/group-focus-within:opacity-100/);
 for(const button of expanded)assert.equal(nodes(button.props.children,n=>n.type==='button',true).length,0,'no nested buttons');
 let stopped=0;play.props.onClick({stopPropagation(){stopped++;}});play.props.onClick({stopPropagation(){stopped++;}});assert.equal(stopped,2);assert.equal(opens,0);
 props.active=false;render();resolve();await flush();assert.equal(plays,0,'late reply after leaving view cannot play');
 props.active=true;let fresh=render();const next=nodes(fresh,n=>n.type==='button'&&n.props['aria-label']===i18n.t('common.playAlbum'),true)[0];next.props.onClick({stopPropagation(){}});resolve();await flush();assert.equal(plays,1);assert.equal(opens,0);
 const card=nodes(fresh,n=>n.props?.['data-discover-item']===presentation.discoverItemKey(props.session.getSnapshot().hubs[0].items[0]),true)[0];card.props.onClick({currentTarget:{}});assert.equal(opens,1);assert.equal(plays,1);
 props.isPlaybackAvailable=false;fresh=render();assert.equal(nodes(fresh,n=>n.type==='button'&&n.props['aria-label']===i18n.t('common.playAlbum'),true)[0].props.disabled,true);
});


test('album submission errors are visible and retryable without leaking backend text',async()=>{
 const props={...await fixture(),isPlaybackAvailable:true,onPlayAlbum:async()=>{throw Error('private backend details');}};
 const state={refs:[],cursor:0,effects:[],values:[],valueCursor:0},render=()=>{state.cursor=0;state.valueCursor=0;host=state;const tree=View(props);host=null;return tree;};
 const tree=render();nodes(tree,n=>n.type==='button'&&n.props['aria-label']===i18n.t('common.playAlbum'),true)[0].props.onClick({stopPropagation(){}});await flush();
 const current=render();assert.equal(nodes(current,n=>n.props?.role==='alert').length,1);
 const html=renderToStaticMarkup(current);assert.match(html,/album could not be played/);assert.doesNotMatch(html,/private backend details/);
 assert.equal(nodes(current,n=>n.type==='button'&&n.props['aria-label']===i18n.t('common.playAlbum'),true)[0].props.disabled,false);
});

test('actual mix artist/album buttons keep source, selection, mode, scroll and keyboard focus through existing page callbacks',async()=>{
 for(const selection of ['7','all'])for(const mode of ['library','decade','style'])for(const language of ['en','pt-BR']){
  await i18n.changeLanguage(language);let queries=0,plays=0,saves=0;
  const libraries=[{key:'7',title:'Music seven'},{key:'8',title:'Music eight'}],s={...scope,identity:{},libraryId:selection,libraries};
  const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail('navigation is not a Discover children lookup'),async(server_id,library_id,language)=>({server_id,library_id,language,items:[],partial:false,dates_missing:false}),()=>0,undefined,
   async(server_id,library_id,language)=>{queries++;return{server_id,library_id,language,tracks:[{server_id,library_id,rating_key:'99',title:'Same track title',artist:'A full artist name that remains available beyond truncation',album_title:'A full album title that remains available beyond truncation',album_rating_key:'12',artist_rating_key:'13',media_locator:{kind:'plex',server_id,rating_key:'99',part_key:'/library/parts/99/file.flac'}}],partial:false,has_more:false};},
   async(server_id,library_id,language)=>({server_id,library_id,language,years:[1992],partial:false}),
   async(server_id,library_id,language)=>({server_id,library_id,language,field:'artist.style',choices:[{id:'55',title:'Jazz'}],partial:false}));
  session.setContext(s);session.ensure();await flush();
  if(mode==='decade'){session.openDecades();await flush();session.chooseDecade(1990);}
  else if(mode==='style'){session.openStyles();await flush();session.chooseStyle(session.styleOptions()[0].key);}
  else session.openMix();await flush();
  const selected=session.getSnapshot().mix.tracks,order=selected.map(t=>[t.server_id,t.library_id,t.rating_key]),style=session.getSnapshot().mixStyle,decade=session.getSnapshot().mixDecade;assert.ok(selected.length);
  const opened=[],focus=[],h={refs:[],cursor:0,effects:[]};let targets=[];
  const props={session,scope:s,active:true,selection,libraries,librariesLoading:false,librariesError:false,onLibraryChange(){},onLibrary(){},onRetryLibraries(){},onOpenAlbum:album=>opened.push({kind:'album',item:album}),onOpenArtist:artist=>opened.push({kind:'artist',item:artist}),onPlayMix:async()=>plays++,onSaveMix:async()=>saves++,isPlaybackAvailable:true};
  const viewport={scrollTop:0,querySelector:()=>({focus(){}}),querySelectorAll:selector=>selector==='[data-mix-link]'?targets:[]};
  const render=()=>{h.cursor=0;h.effects=[];host=h;const tree=View(props);host=null;h.refs[0].current=viewport;h.effects.forEach(fn=>fn());return tree;};
  let tree=render();const before=queries;
  for(const kind of ['album','artist']){
   const row=selected.at(-1),key=JSON.stringify([row.server_id,row.library_id,row.rating_key,kind]);
   const link=nodes(tree,n=>n.props?.['data-mix-link']===key)[0];assert.ok(link);assert.equal(link.type,'button');assert.equal(link.props.type,'button');assert.equal(link.props.disabled,false);
   const title=kind==='artist'?row.artist:row.album_title;assert.equal(link.props.title,title);assert.equal(link.props['aria-label'],i18n.t('discover.openItem',{title}));
   nodes(tree,n=>n.props?.onScroll)[0].props.onScroll({currentTarget:{scrollTop:653}});
   const target={isConnected:kind==='album',dataset:{mixLink:key},focus:options=>focus.push([key,options])};targets=[{dataset:{mixLink:key},focus:options=>focus.push([key,options])}];
   // Native buttons dispatch this same click with detail=0 for Enter/Space.
   link.props.onClick({currentTarget:target,detail:0});assert.equal(opened.at(-1).kind,kind);assert.equal(opened.at(-1).item.rating_key,kind==='album'?'12':'13');assert.equal(opened.at(-1).item.server_id,s.serverId);assert.equal(opened.at(-1).item.library_id,row.library_id);
   props.active=false;render();viewport.scrollTop=0;props.active=true;tree=render();assert.equal(viewport.scrollTop,653);assert.deepEqual(focus.at(-1),[key,{preventScroll:true}]);
   assert.equal(session.getSnapshot().mix.tracks,selected);assert.deepEqual(selected.map(t=>[t.server_id,t.library_id,t.rating_key]),order);assert.equal(session.getSnapshot().mixMode,mode);assert.equal(session.getSnapshot().mixStyle,style);assert.equal(session.getSnapshot().mixDecade,decade);
  }
  assert.equal(queries,before,'no enrichment, regeneration, or lookup on link render/navigation/back');assert.equal(plays,0);assert.equal(saves,0);
 }
 await i18n.changeLanguage('en');
});

test('actual mix keeps missing/invalid parent identities as plain text and rejects stale or inactive callbacks',async()=>{
 let calls=0;const session=new DiscoverSession(async(server_id,library_id,language)=>({server_id,library_id,language,hubs:[],partial:false}),async()=>assert.fail(),undefined,()=>0,undefined,async(server_id,library_id,language)=>({server_id,library_id,language,tracks:[
  {server_id,library_id,rating_key:'1',title:'Missing',artist:'Same artist',album_title:'Same album',media_locator:{kind:'plex',server_id,rating_key:'1',part_key:'/library/parts/1/file'}},
  {server_id,library_id,rating_key:'2',title:'Invalid',artist:'Same artist',album_title:'Same album',album_rating_key:'../12',artist_rating_key:'13?token=private',media_locator:{kind:'plex',server_id,rating_key:'2',part_key:'/library/parts/2/file'}},
  {server_id,library_id,rating_key:'3',title:'Known',artist:'Same artist',album_title:'Same album',album_rating_key:'12',artist_rating_key:'13',media_locator:{kind:'plex',server_id,rating_key:'3',part_key:'/library/parts/3/file'}}],has_more:false,partial:false}));
 session.setContext(scope);session.ensure();await flush();session.openMix();await flush();
 const props={...await fixture(),session,onOpenAlbum:()=>calls++,onOpenArtist:()=>calls++};const h={refs:[],cursor:0,effects:[]};const render=()=>{h.cursor=0;h.effects=[];host=h;const tree=View(props);host=null;return tree;};
 let tree=render();assert.equal(session.getSnapshot().mix.tracks.length,3);const links=nodes(tree,n=>n.props?.['data-mix-link']);assert.equal(links.length,2);assert.ok(links.every(n=>JSON.parse(n.props['data-mix-link'])[2]==='3'));
 const html=renderToStaticMarkup(React.createElement(View,props));assert.match(html,/Missing/);assert.match(html,/Invalid/);assert.doesNotMatch(html,/token=private|\.\.\/12/);
 const target={isConnected:true};props.active=false;render();links[0].props.onClick({currentTarget:target});assert.equal(calls,0);
 props.active=true;tree=render();session.openMixSave();links[0].props.onClick({currentTarget:target});assert.equal(calls,0);session.cancelMixSave();
 session.setContext({...scope,identity:{}});links[1].props.onClick({currentTarget:target});assert.equal(calls,0);
});
