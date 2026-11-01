import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import {navidromeQueueTrack} from '../src/utils/navidromeTracks.ts';
import * as presentation from '../src/utils/albumPresentation.ts';
import {NavidromeAlbumFeed,emptyNavidromeFeed} from '../src/utils/navidromeAlbumFeed.ts';
import {currentMediaKey} from '../src/utils/playerNavigation.ts';
import {queueItemIsCurrent,playConfirmedQueuePosition} from '../src/utils/nowPlayingPresentation.ts';
const read=file=>readFileSync(new URL('../'+file,import.meta.url),'utf8');
function compile(file,deps={}){const exports={};new Function('require','exports',ts.transpileModule(read('src/'+file),{fileName:file,compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);return exports;}
const {navidromeCovers}=compile('utils/navidromeCovers.ts');
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const connection={connection_id:'fixture-connection',account_id:'a'.repeat(64),server:'http://fixture.invalid/',username:'fixture'};
const album={id:'album-a',title:'Album A',artist:'Artist',year:2020,cover_id:null,song_count:2};
const page={albums:[album,{...album,id:'album-b',title:'Album B'}],offset:0,more:false};
const detail={album,songs:[{id:'song-a',title:'Track A',artist:'Artist',album:'Album A',duration:60,disc:1,track:1}]};
function nodes(tree,predicate,result=[]){if(!tree||typeof tree!=='object')return result;if(Array.isArray(tree)){tree.forEach(n=>nodes(n,predicate,result));return result;}if(predicate(tree))result.push(tree);nodes(tree.props?.children,predicate,result);return result;}
function text(tree){if(tree==null||typeof tree==='boolean')return '';if(typeof tree==='string'||typeof tree==='number')return String(tree);if(Array.isArray(tree))return tree.map(text).join(' ');return text(tree.props?.children);}
async function fixture(options={}){
 const language=options.language??'en',i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read(`src/locales/${language}.json`))}},interpolation:{escapeValue:false}});const t=i18n.t.bind(i18n);
 let cursor=0,disposed=false,configurationChanged,unsubscribed=0;const slots=[],effects=[],calls=[];
 const react={...React,default:React,useState(initial){const n=cursor++;if(!(n in slots))slots[n]=typeof initial==='function'?initial():initial;return[slots[n],value=>{assert.equal(disposed,false,'no state write after unmount');slots[n]=typeof value==='function'?value(slots[n]):value}]},useRef(initial){return slots[cursor++]??={current:initial}},useEffect(fn,deps){const n=cursor++,prior=slots[n];if(!prior||deps.some((v,i)=>!Object.is(v,prior.deps[i])))effects.push(()=>{prior?.cleanup?.();slots[n]={deps,setup:fn,cleanup:fn()};})}};
 const service={subscribeConfiguration:async fn=>{configurationChanged=fn;return options.subscription?options.subscription(fn):()=>unsubscribed++;},status:async()=>{calls.push(['status']);return options.status?options.status():connection;},albums:async(...args)=>{calls.push(['albums',...args]);return options.albums?options.albums(...args):page;},album:async(...args)=>{calls.push(['album',...args]);return options.album?options.album(...args):detail;},connect:async(...args)=>{calls.push(['connect',...args]);return options.connect?options.connect(...args):connection;},disconnect:async()=>{calls.push(['disconnect']);return{connection_id:null,server:null,username:null};},play:async(...args)=>{calls.push(['play',...args]);if(options.play)return options.play(...args);},favorites:async(...args)=>{calls.push(['favorites',...args]);return options.favorites?options.favorites(...args):[];},setFavorite:async(...args)=>{calls.push(['setFavorite',...args]);return options.setFavorite?options.setFavorite(...args):[];},cover:async()=>''};
 const intent=()=>({current:()=>true,confirmCurrent:async()=>true,cancel(){calls.push(['cancelIntent']);}});
 const shared={'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t,i18n})},'../utils/albumPresentation':{...presentation,readAlbumPresentation:()=>options.presentation??presentation.DEFAULT_ALBUM_PRESENTATION,saveAlbumPresentation:(...args)=>calls.push(['presentation',...args])}};
 const {AlbumViewControls}=compile('components/AlbumViewControls.tsx',shared),{AlbumCatalogRow}=compile('components/AlbumCatalogRow.tsx',shared);
 const AlbumCatalog=({items,renderItem})=>jsx.jsx('div',{children:items.map(renderItem)});
 const ToolbarSelect=({label,value,options,onChange,disabled})=>jsx.jsx('select',{'aria-label':label,value,disabled,onChange:event=>onChange(event.target.value),children:options.map(option=>jsx.jsx('option',{value:option.value,children:option.label},option.value))});
 const expand=node=>Array.isArray(node)?node.map(expand):!node||typeof node!=='object'?node:[AlbumCatalog,AlbumViewControls,AlbumCatalogRow,ToolbarSelect].includes(node.type)?expand(node.type(node.props)):node.props?.children?{...node,props:{...node.props,children:expand(node.props.children)}}:node;
 const componentName=options.settings?'NavidromeConnectionSetting':'NavidromeView';
 const {[componentName]:NavidromeView}=compile(`components/${componentName}.tsx`,{react,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t})},'../services/navidrome':{navidromeService:service,navidromeError:e=>typeof e==='string'?({NAVIDROME_AUTH:'auth',NAVIDROME_PROTOCOL:'protocol',NAVIDROME_CONTEXT_CHANGED:'changed'}[e]??'unavailable'):'unavailable'},'../utils/navidromeArtwork':{navidromeAlbumCover:navidromeCovers(service.cover)},'../utils/playbackIntent':{beginPlaybackIntent:intent,isPlaybackSuperseded:e=>e==='PLAYBACK_SUPERSEDED',playbackFailure:e=>({PLAYBACK_QUEUE_UNCERTAIN:'uncertain',PLAYBACK_PERSISTENCE_FAILED:'persistence',FIRST_RUN_CATALOG_INDEXING:'initialIndexing',LOCAL_ADDITION_INDEXING:'initialIndexing'}[e]??'failed')},'../utils/frontendDiagnostics':{logError(){}},'./PlaylistPickerModal':{PlaylistPickerModal:function PlaylistPickerModal(){return null}},'../utils/navidromeTracks':{navidromeQueueTrack},'./deferredViews':{NavidromeArtistView:function NavidromeArtistView(){return null}},'./NavidromeArtistLink':{NavidromeArtistLink:function NavidromeArtistLink(){return null}},'./AlbumCatalog':{AlbumCatalog},'./AlbumCatalogRow':{AlbumCatalogRow},'./AlbumViewControls':{AlbumViewControls},'./ToolbarSelect':{ToolbarSelect},'../utils/albumPresentation':shared['../utils/albumPresentation'],'../utils/navidromeAlbumFeed':{NavidromeAlbumFeed,emptyNavidromeFeed}});
 const render=()=>{cursor=0;const tree=NavidromeView({available:true,...options.props});effects.splice(0).forEach(fn=>fn());return expand(tree);};
 const button=(tree,key)=>{const found=nodes(tree,n=>n.type==='button'&&(text(n).trim()===t(key)||n.props['aria-label']===t(key)));assert.equal(found.length,1,key);return found[0]};
 const field=(tree,key)=>{const label=nodes(tree,n=>n.type==='label'&&text(n.props.children?.[0])===t(key))[0];assert.ok(label,key);return nodes(label,n=>n.type==='input')[0]};
 return{render,calls,t,button,field,configurationChanged(){assert.ok(configurationChanged,"mounted view must follow connection changes");configurationChanged();},unsubscribed:()=>unsubscribed,async settle(){let tree;for(let i=0;i<5;i++){tree=render();await flush();}return tree;},dispose(){slots.forEach(slot=>slot?.cleanup?.());disposed=true;}};
}
for(const language of ['en','pt-BR'])test(`unconfigured Navidrome offers the Sources shortcut, without credentials, catalog or playback (${language})`,async()=>{
 const f=await fixture({language,status:async()=>({connection_id:null,server:null,username:null})});try{const tree=await f.settle();assert.ok(f.button(tree,'sources.configure'));assert.equal(nodes(tree,n=>n.type==='input'&&n.props.type==='password').length,0);assert.deepEqual(f.calls,[['status']]);assert.ok(!text(tree).includes(f.t('navidrome.empty')));assert.ok(!text(tree).includes('navidrome.'));}finally{f.dispose()}
});
test('unavailable, empty, and loading catalogs remain distinct; retry does not re-read or overwrite connection settings',async()=>{
 let count=0;const pending=deferred();const f=await fixture({albums:async()=>{count++;if(count===1)throw 'PRIVATE http://secret/?password=token';if(count===2)return{albums:[],offset:0,more:false};return pending.promise;}});try{let tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.errors.unavailable')));assert.ok(!text(tree).includes('PRIVATE'));assert.ok(!text(tree).includes(f.t('navidrome.empty')));f.button(tree,'navidrome.retry').props.onClick();tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.empty')));assert.equal(f.calls.filter(c=>c[0]==='status').length,1);f.button(tree,'navidrome.refresh').props.onClick();tree=f.render();tree=f.render();assert.ok(text(tree).includes(f.t('navidrome.loading')));assert.ok(!text(tree).includes(f.t('navidrome.empty')));pending.resolve(page);await flush();}finally{f.dispose()}
});
test('late catalog response cannot overwrite the latest explicit server search',async()=>{
 const old=deferred();const f=await fixture({albums:async(conn,query)=>query?{...page,albums:[{...album,title:'Latest search'}]}:old.promise});try{let tree=await f.settle();const input=nodes(tree,n=>n.type==='input'&&n.props['aria-label']===f.t('navidrome.search'))[0];input.props.onChange({target:{value:'latest'}});tree=f.render();const form=nodes(tree,n=>n.type==='form')[0];form.props.onSubmit({preventDefault(){}});tree=await f.settle();assert.ok(text(tree).includes('Latest search'));old.resolve(page);await flush();tree=f.render();assert.ok(text(tree).includes('Latest search'));assert.ok(!text(tree).includes('Album B'));assert.equal(f.calls.filter(c=>c[0]==='play').length,0);}finally{f.dispose()}
});
test('old album detail cannot replace a later visit, and navigation does not start playback',async()=>{
 const old=deferred();const f=await fixture({album:async(conn,id)=>id==='album-a'?old.promise:{...detail,album:{...album,id:'album-b',title:'New album'}}});try{let tree=await f.settle();nodes(tree,n=>n.type==='button'&&text(n).includes('Album A'))[0].props.onClick();tree=f.render();f.button(tree,'navidrome.back').props.onClick();tree=f.render();nodes(tree,n=>n.type==='button'&&text(n).includes('Album B'))[0].props.onClick();tree=await f.settle();old.resolve(detail);await flush();tree=f.render();assert.ok(text(tree).includes('New album'));assert.equal(f.calls.filter(c=>c[0]==='play').length,0);}finally{f.dispose()}
});
test('connect requires an explicit submission, rejects double submission, and clears the password after success',async()=>{
 const pending=deferred();const f=await fixture({settings:true,status:async()=>({connection_id:null,server:null,username:null}),connect:()=>pending.promise});try{let tree=await f.settle();for(const [key,value]of [['navidrome.server','http://fixture.invalid/'],['navidrome.username','fixture'],['navidrome.password','fixture-password']]){f.field(tree,key).props.onChange({target:{value}});tree=f.render();}const connect=f.button(tree,'navidrome.connect');connect.props.onClick();connect.props.onClick();assert.equal(f.calls.filter(c=>c[0]==='connect').length,1);pending.resolve(connection);tree=await f.settle();assert.equal(f.field(tree,'navidrome.password').props.value,'');assert.equal(f.calls.filter(c=>c[0]==='play').length,0);}finally{f.dispose()}
});
test('leaving Navidrome cancels only its pending playback preparation and ignores late detail/catalog replies',async()=>{
 const pending=deferred();const f=await fixture({play:()=>pending.promise});let tree=await f.settle();nodes(tree,n=>n.type==='button'&&text(n).includes('Album A'))[0].props.onClick();tree=await f.settle();f.button(tree,'navidrome.playAlbum').props.onClick();f.button(tree,'navidrome.playAlbum').props.onClick();assert.equal(f.calls.filter(c=>c[0]==='play').length,1);f.dispose();assert.equal(f.calls.filter(c=>c[0]==='cancelIntent').length,1);pending.resolve();await flush();
 const late=deferred();const f2=await fixture({albums:()=>late.promise});await f2.settle();f2.dispose();late.resolve(page);await flush();
});
test('Navidrome row identity is source-bound and revalidated before playing a queue position',async()=>{
 const media={kind:'navidrome',connection_id:'one',song_id:'song',queue_index:0};const track={title:'Track',artist:'Artist',album:'Album',media_locator:{kind:'navidrome',connection_id:'one',song_id:'song'}};assert.equal(queueItemIsCurrent(track,0,media),true);assert.equal(queueItemIsCurrent(track,0,{...media,connection_id:'two'}),false);assert.notEqual(currentMediaKey(media),currentMediaKey({...media,connection_id:'two'}));let played=0;await playConfirmedQueuePosition([track],0,async()=>[track],async()=>{played++},()=>true);assert.equal(played,1);await assert.rejects(playConfirmedQueuePosition([track],0,async()=>[{...track,media_locator:{...track.media_locator,connection_id:'two'}}],async()=>{played++},()=>true));assert.equal(played,1);
});
test('artwork requests are shared, limited to three in flight, isolated by connection and reject non-image payloads',async()=>{
 const waiting=[],calls=[];const cover=navidromeCovers(async(conn,id)=>{calls.push([conn,id]);const d=deferred();waiting.push(d);return d.promise;});const first=cover('a','cover');assert.equal(cover('a','cover'),first);const rest=Array.from({length:6},(_,i)=>cover('a',String(i)));assert.equal(calls.length,3);for(let i=0;i<7;i++){waiting[i].resolve('data:image/png;base64,AA==');await flush();}await Promise.all([first,...rest]);assert.equal(calls.length,7);await cover('a','cover');assert.equal(calls.length,7);const other=cover('b','cover');assert.equal(calls.length,8);waiting[7].resolve('data:image/png;base64,AA==');await other;await assert.rejects(navidromeCovers(async()=>'<svg onload="PRIVATE"/>')('c','d'),e=>e==='NAVIDROME_PROTOCOL');
});
test('native service keeps playback intents and Direct-choice admission before requesting Navidrome playback',async()=>{
 const calls=[];const {navidromeService}=compile('services/navidrome.ts',{'@tauri-apps/api/core':{invoke:async(...args)=>{calls.push(['invoke',...args]);}},'./config':{ensureDirectOutputChoice:async()=>calls.push(['choice'])},'../utils/localSourceReadQueue':{localSourceReadQueue:{control:async work=>{calls.push(['control']);return work();}}},'../utils/playbackIntent':{playbackIntentToken:async()=>{calls.push(['intent']);return 'fixture-intent';}}});await navidromeService.play('conn','album',0,{current:()=>true});assert.deepEqual(calls.map(c=>c[0]),['intent','control','choice','invoke']);assert.deepEqual(calls.at(-1),['invoke','navidrome_play_album',{connectionId:'conn',id:'album',startIndex:0,intent:'fixture-intent'}]);calls.length=0;await assert.rejects(navidromeService.play('conn','album',0,{current:()=>false}),e=>e==='PLAYBACK_SUPERSEDED');assert.equal(calls.some(c=>c[0]==='invoke'||c[0]==='choice'),false);
});

test('abandoned cover cards are removed before a new page and a shared live consumer keeps its request',async()=>{
 const waiting=[],calls=[];let oldLive=true;const cover=navidromeCovers(async(conn,id)=>{calls.push(id);const d=deferred();waiting.push(d);return d.promise;});
 const active=[0,1,2].map(id=>cover('a',String(id)));
 const abandoned=cover('a','old',()=>oldLive);const cancelled=assert.rejects(abandoned,e=>e==='NAVIDROME_CANCELLED');
 const shared=cover('a','shared',()=>oldLive);assert.equal(cover('a','shared',()=>true),shared);oldLive=false;
 const visible=cover('a','visible');await cancelled;
 for(let i=0;i<5;i++){waiting[i].resolve('data:image/png;base64,AA==');await flush();}
 await Promise.all([...active,shared,visible]);assert.deepEqual(calls,['0','1','2','shared','visible']);
});

test('scrolling appends albums without hiding existing cards, and an empty last batch ends loading',async()=>{
 const pending=deferred();const f=await fixture({albums:async(conn,query,sort,offset)=>offset===0?{...page,more:true}:pending.promise});
 try{let tree=await f.settle();assert.equal(nodes(tree,n=>n.type==='select'&&n.props['aria-label']===f.t('navidrome.perPage')).length,0);
  const scroll=nodes(tree,n=>n.type==='div'&&n.props.onScroll)[0];scroll.props.onScroll({currentTarget:{scrollTop:900,scrollHeight:1400,clientHeight:500}});
  tree=f.render();assert.ok(text(tree).includes('Album A'));assert.ok(text(tree).includes(f.t('navidrome.loading')));assert.deepEqual(f.calls.filter(c=>c[0]==='albums').at(-1).slice(4),[2,36]);
  pending.resolve({albums:[],offset:2,more:false});tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.ok(!text(tree).includes(f.t('navidrome.empty')));assert.equal(nodes(tree,n=>n.type==='button'&&text(n)===f.t('navidrome.loadMore')).length,0);
 }finally{f.dispose();}
});

for(const [code,key]of [['FIRST_RUN_CATALOG_INDEXING','initialIndexing'],['PLAYBACK_QUEUE_UNCERTAIN','uncertain'],['PLAYBACK_PERSISTENCE_FAILED','persistence']])test(`Navidrome preserves the core playback contract for ${code} instead of blaming the server`,async()=>{
 const f=await fixture({play:async()=>{throw code;}});try{let tree=await f.settle();nodes(tree,n=>n.type==='button'&&text(n).includes('Album A'))[0].props.onClick();tree=await f.settle();f.button(tree,'navidrome.playAlbum').props.onClick();await flush();tree=f.render();assert.ok(text(tree).includes(f.t(`player.requestError.${key}`)));assert.ok(!text(tree).includes(f.t('navidrome.errors.unavailable')));assert.equal(f.calls.filter(c=>c[0]==='play').length,1);}finally{f.dispose()}
});


test('a failed append retains cards, retries the same offset once and does not retry on scroll',async()=>{
 let attempts=0;const f=await fixture({albums:async(conn,q,sort,offset)=>{if(!offset)return{...page,more:true};if(++attempts===1)throw 'NAVIDROME_UNAVAILABLE';return{albums:[{...album,id:'album-c',title:'Album C'}],offset,more:false};}});
 try{let tree=await f.settle();f.button(tree,'navidrome.loadMore').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.ok(text(tree).includes(f.t('navidrome.errors.unavailable')));
  nodes(tree,n=>n.type==='div'&&n.props.onScroll)[0].props.onScroll({currentTarget:{scrollTop:900,scrollHeight:1400,clientHeight:500}});await flush();assert.equal(attempts,1);
  f.button(tree,'navidrome.retry').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.ok(text(tree).includes('Album C'));assert.deepEqual(f.calls.filter(c=>c[0]==='albums').map(c=>c[4]),[0,2,2]);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);
 }finally{f.dispose();}
});
test('changing Navidrome view keeps catalog and playback untouched; details do not mount artwork',async()=>{
 const f=await fixture();try{let tree=await f.settle();const requests=f.calls.filter(c=>c[0]==='albums').length;
  const views=()=>nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('albumViews.details'));
  views()[0].props.onClick();tree=await f.settle();assert.equal(f.calls.filter(c=>c[0]==='albums').length,requests);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);assert.ok(text(tree).includes('Album A'));
  assert.equal(nodes(tree,n=>typeof n.type==='function'&&n.type.name==='Artwork').length,0);assert.equal(f.calls.filter(c=>c[0]==='presentation').at(-1)[1],'navidrome');
  nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('albumViews.list'))[0].props.onClick();tree=await f.settle();assert.equal(nodes(tree,n=>typeof n.type==='function'&&n.type.name==='Artwork').length,2);
  nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('albumViews.play',{title:'Album A'}))[0].props.onClick();await flush();assert.equal(f.calls.filter(c=>c[0]==='play').length,1);
 }finally{f.dispose();}
});
test('favoriting a catalog album is explicit, rejects duplicate clicks and publishes the saved native result',async()=>{
 const pending=deferred();const f=await fixture({setFavorite:()=>pending.promise});try{let tree=await f.settle();
  const heart=nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('common.addFavorite'))[0];assert.equal(heart.props['aria-pressed'],false);
  heart.props.onClick({stopPropagation(){}});heart.props.onClick({stopPropagation(){}});assert.equal(f.calls.filter(c=>c[0]==='setFavorite').length,1);assert.deepEqual(f.calls.find(c=>c[0]==='setFavorite').slice(1),['fixture-connection','album-a',true]);
  pending.resolve([album]);tree=await f.settle();assert.equal(nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('favorites.removeFavorite'))[0].props['aria-pressed'],true);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);
 }finally{f.dispose();}
});
test('Sonante Navidrome favorites browse locally, open server details only on request and preserve ordinary playback intent',async()=>{
 const counts=[];const f=await fixture({props:{favoritesOnly:true,embedded:true,onFavoritesChange:n=>counts.push(n)},favorites:async()=>[album]});
 try{let tree=await f.settle();assert.equal(f.calls.filter(c=>c[0]==='albums').length,0);assert.ok(text(tree).includes('Album A'));assert.deepEqual(counts,[1]);
  nodes(tree,n=>n.type==='button'&&text(n)==='Album A')[0].props.onClick();tree=await f.settle();assert.equal(f.calls.filter(c=>c[0]==='album').length,1);f.button(tree,'navidrome.playAlbum').props.onClick();await flush();assert.equal(f.calls.filter(c=>c[0]==='play').length,1);
 }finally{f.dispose();}
});
test('failed favorites read preserves catalog browsing and exposes retry without offering destructive favorite mutations',async()=>{
 let attempt=0;const f=await fixture({favorites:async()=>{if(++attempt===1)throw 'NAVIDROME_STORAGE';return[album];}});try{let tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.ok(text(tree).includes(f.t('navidrome.favoritesFailed')));assert.ok(nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('common.addFavorite')).every(n=>n.props.disabled));f.button(tree,'navidrome.retryFavorites').props.onClick();tree=await f.settle();assert.equal(nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('favorites.removeFavorite')).length,1);assert.equal(f.calls.filter(c=>c[0]==='setFavorite').length,0);}finally{f.dispose();}
});
test('pagination and Sonante favorites service send only bounded selections and IDs to native commands',async()=>{
 const calls=[];const {navidromeService}=compile('services/navidrome.ts',{'@tauri-apps/api/core':{invoke:async(...args)=>{calls.push(args);return[]}},'./config':{},'../utils/localSourceReadQueue':{},'../utils/playbackIntent':{}});
 await navidromeService.albums('conn','search','name',72,18);await navidromeService.favorites('conn');await navidromeService.setFavorite('conn','album',false);
 assert.deepEqual(calls,[['navidrome_albums',{connectionId:'conn',query:'search',sort:'name',offset:72,pageSize:18}],['navidrome_favorites',{connectionId:'conn'}],['navidrome_set_favorite',{connectionId:'conn',id:'album',enabled:false}]]);
});

test('catalog append does not replace an open album, and Back restores the loaded list and scroll position',async()=>{
 const pending=deferred();const f=await fixture({albums:async(conn,q,sort,offset)=>offset===0?{...page,more:true}:pending.promise});
 try{let tree=await f.settle();const container={scrollTop:700,scrollHeight:1200,clientHeight:500,scrollTo({top}){this.scrollTop=top;}};
  const scroll=nodes(tree,n=>n.type==='div'&&n.props.onScroll)[0];scroll.props.ref(container);tree=f.render();container.scrollTop=700;
  f.button(tree,'navidrome.loadMore').props.onClick();tree=f.render();nodes(tree,n=>n.type==='button'&&text(n)==='Album A')[0].props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Track A'));
  pending.resolve({albums:[{...album,id:'album-c',title:'Album C'}],offset:2,more:false});tree=await f.settle();assert.ok(text(tree).includes('Track A'));assert.ok(!text(tree).includes('Album C'));
  f.button(tree,'navidrome.back').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album C'));assert.equal(container.scrollTop,700);assert.equal(f.calls.filter(c=>c[0]==='albums').length,2);
 }finally{f.dispose();}
});
test('Navidrome favorites use all saved matching albums without server pages',async()=>{
 const saved=Array.from({length:80},(_,i)=>({...album,id:'saved-'+i,title:'Saved '+i}));const f=await fixture({props:{favoritesOnly:true,embedded:true},favorites:async()=>saved});
 try{const tree=await f.settle();assert.ok(text(tree).includes('Saved 79'));assert.equal(f.calls.filter(c=>c[0]==='albums').length,0);assert.equal(nodes(tree,n=>n.type==='nav').length,0);}finally{f.dispose();}
});

test('library total comes from the server and stays independent from accumulated scroll rows',async()=>{
 const f=await fixture({albums:async(conn,q,sort,offset)=>offset===0?{...page,total:500,more:true}:{albums:[{...album,id:'c',title:'C'}],offset,total:500,more:false}});
 try{let tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.libraryCount',{count:500})));f.button(tree,'navidrome.loadMore').props.onClick();tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.libraryCount',{count:500})));assert.equal(f.calls.filter(c=>c[0]==='albums').length,2);}finally{f.dispose();}
});
test('Navidrome album and single-track playlist actions pass typed account/song references without starting playback',async()=>{
 const f=await fixture();try{let tree=await f.settle();nodes(tree,n=>n.type==='button'&&text(n)==='Album A')[0].props.onClick();tree=await f.settle();f.button(tree,'playlists.addAlbum').props.onClick();tree=f.render();let picker=nodes(tree,n=>n.type?.name==='PlaylistPickerModal')[0];assert.equal(picker.props.items.length,1);assert.deepEqual(picker.props.items[0].media_locator,{kind:'navidrome',connection_id:'fixture-connection',account_id:'a'.repeat(64),song_id:'song-a'});picker.props.onClose();tree=f.render();f.button(tree,'playlists.addTrack').props.onClick();tree=f.render();picker=nodes(tree,n=>n.type?.name==='PlaylistPickerModal')[0];assert.equal(picker.props.items.length,1);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);}finally{f.dispose();}
});
test('album opened from history requests only its matching account detail, without loading the catalog',async()=>{
 let backs=0;const f=await fixture({props:{initialAlbum:{account_id:'a'.repeat(64),id:'album-a'},onAlbumBack(){backs++;}}});try{let tree=await f.settle();assert.ok(text(tree).includes('Track A'));assert.equal(f.calls.filter(c=>c[0]==='albums').length,0);f.button(tree,'navidrome.back').props.onClick();assert.equal(backs,1);}finally{f.dispose();}
 const other=await fixture({props:{initialAlbum:{account_id:'b'.repeat(64),id:'album-a'}}});try{const tree=await other.settle();assert.ok(text(tree).includes(other.t('navidrome.errors.changed')));assert.equal(other.calls.filter(c=>c[0]==='albums'||c[0]==='album').length,0);}finally{other.dispose();}
});


test('same-account Navidrome reconnect keeps the open album, loaded catalog and scroll without replaying',async()=>{
 let status=connection;const f=await fixture({status:async()=>status});try{
  let tree=await f.settle();const viewport=nodes(tree,n=>n.props?.onScroll)[0],container={scrollTop:475,scrollTo({top}){this.scrollTop=top;}};viewport.props.ref(container);tree=f.render();container.scrollTop=475;viewport.props.onScroll({currentTarget:container});
  nodes(tree,n=>n.type==='button'&&text(n)==='Album B')[0].props.onClick();tree=await f.settle();const count=f.calls.filter(c=>c[0]==='albums').length;
  status={...connection,connection_id:'replacement-session'};f.configurationChanged();tree=await f.settle();assert.ok(text(tree).includes('Track A'));assert.equal(f.calls.filter(c=>c[0]==='albums').length,count);
  assert.deepEqual(f.calls.filter(c=>c[0]==='album').at(-1),['album','replacement-session','album-b']);f.button(tree,'navidrome.back').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album B'));assert.equal(container.scrollTop,475);assert.equal(f.calls.some(c=>c[0]==='play'||c[0]==='disconnect'||c[0]==='connect'),false);
 }finally{f.dispose();assert.equal(f.unsubscribed(),1)}
});
test('another Navidrome account replaces only browsing state and fences late old album/favorite responses',async()=>{
 let status=connection;const old=deferred(),saved=deferred();const f=await fixture({status:async()=>status,album:()=>old.promise,favorites:async id=>id===connection.connection_id?saved.promise:[] ,albums:async id=>id===connection.connection_id?page:{albums:[{...album,id:'new-account',title:'New account album'}],offset:0,more:false}});
 try{let tree=await f.settle();nodes(tree,n=>n.type==='button'&&text(n)==='Album A')[0].props.onClick();f.render();status={...connection,connection_id:'new-account-session',account_id:'b'.repeat(64)};f.configurationChanged();tree=await f.settle();old.resolve({...detail,album:{...album,title:'OLD PRIVATE ALBUM'}});saved.resolve([album]);tree=await f.settle();assert.ok(text(tree).includes('New account album'));assert.ok(!text(tree).includes('OLD PRIVATE ALBUM'));assert.equal(f.calls.filter(c=>c[0]==='album').length,1,'old album ID must not be queried in the new account');assert.equal(f.calls.some(c=>c[0]==='play'),false);}finally{f.dispose()}
});
test('failed Navidrome page retries the same offset after network recovery without losing loaded albums or the selected sort',async()=>{
 let reads=0;const f=await fixture({albums:async(_id,_query,_sort,offset)=>{reads++;if(offset===0)return{...page,more:true};if(reads===2)throw 'NAVIDROME_UNAVAILABLE';return{albums:[{...album,id:'third',title:'Recovered album'}],offset,more:false};}});
 try{let tree=await f.settle();f.button(tree,'navidrome.loadMore').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.ok(text(tree).includes(f.t('navidrome.errors.unavailable')));f.button(tree,'navidrome.retry').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Recovered album'));assert.ok(text(tree).includes('Album A'));assert.deepEqual(f.calls.filter(c=>c[0]==='albums').map(c=>c.slice(3)),[['recent',0,36],['recent',2,36],['recent',2,36]]);assert.equal(f.calls.some(c=>c[0]==='play'),false);}finally{f.dispose()}
});
test('reconnect cancels pending playback preparation; status failure keeps navigation and offers retry, not a false unconfigured source',async()=>{
 let status=connection,fail=false;const preparation=deferred();const f=await fixture({status:async()=>{if(fail)throw 'NAVIDROME_STORAGE';return status;},album:()=>preparation.promise});
 try{let tree=await f.settle();nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('navidrome.quickPlay',{title:'Album A'}))[0].props.onClick();await flush();fail=true;f.configurationChanged();tree=await f.settle();preparation.resolve(detail);tree=await f.settle();assert.equal(f.calls.some(c=>c[0]==='play'),false);assert.ok(text(tree).includes('Album A'));assert.ok(!text(tree).includes(f.t('sources.navidromeConfigureHint')));
  fail=false;f.button(tree,'navidrome.retry').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.equal(f.calls.filter(c=>c[0]==='albums').length,1);
 }finally{f.dispose()}
 const broken=await fixture({status:async()=>{throw 'NAVIDROME_STORAGE'}});try{const tree=await broken.settle();assert.ok(text(tree).includes(broken.t('navidrome.errors.unavailable')));assert.ok(!text(tree).includes(broken.t('sources.navidromeConfigureHint')));}finally{broken.dispose()}
});
test('closing Navidrome before event subscription resolves removes the late listener without a status/server read',async()=>{
 const pending=deferred();let stopped=0;const f=await fixture({subscription:()=>pending.promise});f.render();f.dispose();pending.resolve(()=>stopped++);await flush();assert.equal(stopped,1);assert.equal(f.calls.length,0);
});


test('a connection event immediately fences an older status snapshot before the next effect cleanup',async()=>{
 const old=deferred();let reads=0;const next={...connection,connection_id:'current-session',account_id:'b'.repeat(64)};
 const f=await fixture({status:async()=>++reads===1?old.promise:next,albums:async id=>{assert.equal(id,'current-session','old status must not launch a catalog request');return page;}});
 try{await f.settle();f.configurationChanged();old.resolve(connection);await flush();await f.settle();assert.equal(reads,2);assert.ok(f.calls.some(c=>c[0]==='albums'&&c[1]==='current-session'));assert.equal(f.calls.some(c=>c[0]==='play'),false);}finally{f.dispose()}
});
test('disconnecting Navidrome clears only the page; a history destination never queries its old album in another account',async()=>{
 let status=connection;const f=await fixture({status:async()=>status});try{await f.settle();status={connection_id:null,account_id:null,server:null,username:null};f.configurationChanged();const tree=await f.settle();assert.ok(text(tree).includes(f.t('sources.navidromeConfigureHint')));assert.ok(!text(tree).includes('Album A'));assert.equal(f.calls.filter(c=>c[0]==='albums').length,1);assert.equal(f.calls.some(c=>c[0]==='play'||c[0]==='disconnect'),false);}finally{f.dispose()}
 status=connection;const g=await fixture({props:{initialAlbum:{account_id:connection.account_id,id:'album-a'},embedded:true},status:async()=>status});try{await g.settle();status={...connection,connection_id:'different-session',account_id:'b'.repeat(64)};g.configurationChanged();const tree=await g.settle();assert.equal(g.calls.filter(c=>c[0]==='album').length,1);assert.ok(text(tree).includes(g.t('navidrome.errors.changed')));assert.equal(g.calls.some(c=>c[0]==='play'),false);}finally{g.dispose()}
});


test('a failed Navidrome refresh keeps visible albums, scroll and query until explicit retry succeeds',async()=>{
 let reads=0;const f=await fixture({albums:async()=>{if(++reads===2)throw 'NAVIDROME_UNAVAILABLE';return reads===1?page:{...page,albums:[{...album,id:'recovered',title:'Recovered snapshot'}]};}});
 try{let tree=await f.settle();const scroll=nodes(tree,n=>n.props?.onScroll)[0],container={scrollTop:310,scrollTo({top}){this.scrollTop=top;}};scroll.props.ref(container);f.render();container.scrollTop=310;scroll.props.onScroll({currentTarget:container});f.button(tree,'navidrome.refresh').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Album A'));assert.equal(container.scrollTop,310);assert.ok(text(tree).includes(f.t('navidrome.errors.unavailable')));f.button(tree,'navidrome.retry').props.onClick();tree=await f.settle();assert.ok(text(tree).includes('Recovered snapshot'));assert.equal(nodes(tree,n=>n.type==='button'&&text(n)==='Album A').length,0);assert.equal(container.scrollTop,310);assert.equal(f.calls.some(c=>c[0]==='play'),false);}finally{f.dispose()}
});

for(const language of ['en','pt-BR'])test(`status failure never masquerades as an empty Navidrome collection (${language})`,async()=>{
 let fail=false;
 const f=await fixture({language,status:async()=>{if(fail)throw 'NAVIDROME_UNAVAILABLE';return connection;},albums:async()=>({albums:[],offset:0,more:false})});
 try{let tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.empty')));
  fail=true;f.configurationChanged();tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.errors.unavailable')));assert.ok(!text(tree).includes(f.t('navidrome.empty')));assert.ok(!text(tree).includes(f.t('sources.navidromeConfigureHint')));
  fail=false;f.button(tree,'navidrome.retry').props.onClick();tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.empty')));assert.equal(f.calls.filter(c=>c[0]==='play').length,0);
 }finally{f.dispose();}
});
test('initial Navidrome catalog request shows one loading state and no empty result before completion',async()=>{
 const pending=deferred();const f=await fixture({albums:()=>pending.promise});
 try{let tree=await f.settle();assert.equal(nodes(tree,n=>n.props?.role==='status'&&text(n).includes(f.t('navidrome.loading'))).length,1);assert.ok(!text(tree).includes(f.t('navidrome.empty')));pending.resolve({albums:[],offset:0,more:false});tree=await f.settle();assert.ok(text(tree).includes(f.t('navidrome.empty')));}finally{f.dispose();}
});

test('Navidrome catalog and album artist destinations keep the original view mounted and Back never reloads or plays',async()=>{
 const f=await fixture();try{let tree=await f.settle();const links=nodes(tree,n=>n.props?.albumId==='album-a'&&n.props?.onOpen);assert.ok(links.length);links[0].props.onOpen({account_id:connection.account_id,id:'artist-1'});tree=f.render();const artist=nodes(tree,n=>n.props?.artist?.id==='artist-1')[0];assert.ok(artist);assert.ok(nodes(tree,n=>n.type==='section'&&n.props.hidden).length);const before=f.calls.length;artist.props.onBack();tree=f.render();assert.equal(f.calls.length,before);assert.ok(nodes(tree,n=>n.type==='button'&&text(n)==='Album A').length);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);}finally{f.dispose()}
});
