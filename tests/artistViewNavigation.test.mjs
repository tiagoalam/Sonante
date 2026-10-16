import test from 'node:test';
import { discoverPush } from '../src/utils/discoverNavigation.ts';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import * as icons from 'lucide-react';
import i18next from 'i18next';
const i18n=i18next.createInstance();await i18n.init({lng:'en',resources:{en:{translation:JSON.parse(readFileSync(new URL('../src/locales/en.json',import.meta.url),'utf8'))}}});
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{resolve,reject,promise};};
const flush=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
let host,plays=0;const calls=[];
const album=(title='Synthetic album')=>({rating_key:'12',artist_rating_key:'88',title,artist:'Synthetic artist',server_id:'server-a',library_id:'2',thumb:null});
const service={getArtistAlbums:async key=>{calls.push(['albums',key]);return[album()];},getArtistTopTracks:async key=>{calls.push(['tracks',key]);return Array.from({length:10},(_,i)=>({rating_key:String(i+1),title:`Synthetic track ${i}`,duration_ms:1000,media_locator:{kind:'plex',server_id:'server-a',part_key:`/library/parts/${i+1}/file`}}));}};
const equal=(a,b)=>a?.length===b?.length&&a.every((v,i)=>Object.is(v,b[i]));
const hooks={
 useState(initial){const index=host.cursor++,h=host;const slot=h.slots[index]??={state:typeof initial==='function'?initial():initial};return[slot.state,value=>{h.writes++;slot.state=typeof value==='function'?value(slot.state):value;}];},
 useRef(initial){const index=host.cursor++;return(host.slots[index]??={current:initial});},
 useEffect(fn,deps){effect(fn,deps,false);},useLayoutEffect(fn,deps){effect(fn,deps,true);}
};
function effect(fn,deps,layout){const index=host.cursor++,h=host,slot=h.slots[index]??={};if(!equal(slot.deps,deps)){(layout?h.layouts:h.effects).push(()=>{slot.cleanup?.();slot.deps=deps;slot.cleanup=fn();});}}
const deps={react:{__esModule:true,...React,...hooks,default:React},'react/jsx-runtime':await import('react/jsx-runtime'),'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'lucide-react':icons,
 '../utils/frontendDiagnostics':{logError(){}},'../services/plex':{plexService:service},'../services/audio':{audioService:{playTracks:async()=>plays++,togglePlay:async()=>plays++}},
 './PlexImage':{PlexImage:()=>React.createElement('div',{'data-synthetic-image':true})},'./PlaylistPickerModal':{PlaylistPickerModal:()=>null}};
const compile=source=>ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};new Function('require','exports',compile(readFileSync(new URL('../src/components/ArtistView.tsx',import.meta.url),'utf8')))(name=>deps[name],exports);
const View=exports.ArtistView;
class Harness{
 slots=[];writes=0;cursor=0;effects=[];layouts=[];
 render(props,commit=true){this.cursor=0;this.effects=[];this.layouts=[];host=this;const tree=(this.component??View)(props);host=null;if(commit){this.layouts.forEach(fn=>fn());this.effects.forEach(fn=>fn());}return tree;}
 unmount(){this.slots.forEach(slot=>slot.cleanup?.());}
}
const nodes=(tree,predicate)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,predicate)):!tree||typeof tree!=='object'?[]:[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)];
const props=()=>({artist:{rating_key:'88',name:'Synthetic artist',server_id:'server-a',library_id:'2'},requestIdentity:{},active:true,onBack(){},onSelectAlbum(){},status:{state:'pause',current_media:null},isPlaybackAvailable:true});

test('shared ArtistView discards stale artist/account replies, resets old metadata before effects, and suppresses post-unmount writes',async()=>{
 const h=new Harness(),a=deferred(),b=deferred(),c=deferred();let requests=0;let p={...props(),loadContent:()=>[a,b,c][requests++].promise};h.render(p);await flush();
 p={...p,artist:{...p.artist,rating_key:'89',library_id:'3'}};h.render(p);await flush();b.resolve({albums:[album('Current artist album')],topTracks:[]});await flush();assert.match(renderToStaticMarkup(h.render(p)),/Current artist album/);
 a.resolve({albums:[album('Stale artist album')],topTracks:[]});await flush();assert.doesNotMatch(renderToStaticMarkup(h.render(p)),/Stale artist album/);
 p={...p,requestIdentity:{}};assert.doesNotMatch(renderToStaticMarkup(h.render(p,false)),/Current artist album/,'old account data is masked before passive effects');h.render(p);await flush();h.unmount();const writes=h.writes;c.resolve({albums:[album('Unmounted account album')],topTracks:[]});await flush();assert.equal(h.writes,writes);
});
test('ArtistView keeps browser features and preserves artist → album → artist scroll/focus without playback or refetch',async()=>{
 calls.length=0;plays=0;const h=new Harness(),opened=[];let p={...props(),onSelectAlbum:a=>opened.push(a)};let tree=h.render(p);await flush();tree=h.render(p);assert.deepEqual(calls,[['albums','88'],['tracks','88']]);
 const focus=[],target={focus:options=>focus.push(options)};const main={scrollTop:0};const viewport=h.slots.find(slot=>slot.current===null);viewport.current=main;
 tree.props.onScroll({currentTarget:{scrollTop:440}});nodes(tree,n=>n.props?.['data-artist-album'])[0].props.onClick({currentTarget:target});assert.equal(opened[0].library_id,'2');assert.equal(plays,0);
 p={...p,active:false};h.render(p);main.scrollTop=0;p={...p,active:true};tree=h.render(p);assert.equal(main.scrollTop,440);assert.deepEqual(focus,[{preventScroll:true}]);assert.equal(calls.length,2);
 let stopped=false;tree.props.onKeyDown({target:{closest:()=>target},stopPropagation(){stopped=true;}});assert.ok(stopped);
 assert.match(renderToStaticMarkup(tree),/Popular Tracks|Discography/);nodes(tree,n=>n.type==='button'&&renderToStaticMarkup(n).includes('Show more'))[0].props.onClick();tree=h.render(p);assert.match(renderToStaticMarkup(tree),/Synthetic track 9/);assert.equal(plays,0);h.unmount();
});
test('ArtistView failures show no previous source data and cleanup before deferred invocation avoids an obsolete request',async()=>{
 const h=new Harness();let reads=0;const p={...props(),loadContent:async()=>{reads++;throw 'PLEX_DISCOVER_CONTEXT';}};h.render(p);await flush();const html=renderToStaticMarkup(h.render(p));assert.match(html,/role="alert"/);assert.doesNotMatch(html,/Synthetic album/);h.unmount();
 const never=new Harness();never.render(p);never.unmount();await flush();assert.equal(reads,1,'unmounted effect never enters its loader');
});
test('scoped ArtistView uses its supplied portrait or fallback without promoting an album cover or reloading on image changes',async()=>{
 const h=new Harness();let reads=0;let p={...props(),artist:{...props().artist,thumb:null},loadContent:async()=>{reads++;return{albums:[{...album(),thumb:{server_id:'server-a',path:'/library/metadata/12/thumb/1'}}],topTracks:[]};}};
 h.render(p);await flush();let tree=h.render(p);assert.equal(nodes(tree,n=>n.props?.image&&n.props.alt===p.artist.name).length,0);
 const thumb={server_id:'server-a',path:'/library/metadata/88/thumb/1'};p={...p,artist:{...p.artist,thumb}};tree=h.render(p);assert.equal(nodes(tree,n=>n.props?.image&&n.props.alt===p.artist.name)[0].props.image,thumb);assert.equal(reads,1);h.unmount();
});
test('App uses the same ArtistView for Discover and retains it across album navigation, with stale-context callbacks rejected',()=>{
 const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8'),source=ts.createSourceFile('App.tsx',app,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let element;
 const visit=node=>{if(ts.isJsxElement(node)&&node.openingElement.tagName.getText(source)==='div'&&node.openingElement.attributes.properties.some(p=>p.name?.getText(source)==='data-plex-discover'))element??=node;ts.forEachChild(node,visit);};visit(source);assert.ok(element);
 const code=compile(`export function Scene(){return (${element.getText(source)});}`);
 const context={discoverIdentity:{},discoverScope:{serverId:'server-a'},discoverAlbum:null,discoverArtist:null,discoverPages:[],playerTarget:null,showCollectionWizard:false,discoverSelectionId:'all',libraries:[],loadingLibraries:false,plexLibrariesError:false,playbackStatus:{},isPlaybackAvailable:true};
 const update=pages=>{context.discoverPages=pages;const top=pages.at(-1);context.discoverAlbum=top?.album?top:null;context.discoverArtist=top?.artist?top:null;};
 const setArtist=value=>update(discoverPush(context.discoverPages,value)),setAlbum=value=>update(value?discoverPush(context.discoverPages,value):context.discoverPages.slice(0,-1));const plexDiscover={isScope:value=>value===context.discoverScope};const ArtistView=()=>null,PlexDiscoverView=()=>null,AlbumView=()=>null;
 const build=()=>{const out={};const bindings={...context,plexDiscover,ArtistView,PlexDiscoverView,AlbumView,setDiscoverArtist:setArtist,setDiscoverAlbum:setAlbum,closeDiscoverArtist:()=>update(context.discoverPages.slice(0,-1)),loadDiscoverArtist:()=>assert.fail('render is not a fetch'),refreshPlexFavorites(){},setPlexDiscoveryAttempt(){},setDiscoverSelection(){},setPlexArea(){}};new Function('require','exports',...Object.keys(bindings),code)(name=>deps[name],out,...Object.values(bindings));return out.Scene();};
 let tree=build();const discover=nodes(tree,n=>n.type===PlexDiscoverView)[0],artist={rating_key:'88',name:'Synthetic',server_id:'server-a',library_id:'2'};discover.props.onOpenArtist(artist);tree=build();let page=nodes(tree,n=>n.type===ArtistView)[0];assert.equal(page.props.artist,artist);assert.equal(page.props.requestIdentity,context.discoverIdentity);
 const sourcePage=page.props;page.props.onSelectAlbum(album());tree=build();page=nodes(tree,n=>n.type===ArtistView)[0];assert.equal(page.props.active,false);assert.equal(page.props.artist,artist);nodes(tree,n=>n.type===AlbumView)[0].props.onBack();tree=build();assert.equal(nodes(tree,n=>n.type===ArtistView)[0].props.active,true);assert.equal(context.discoverArtist.artist,artist);nodes(tree,n=>n.type===ArtistView)[0].props.onBack();assert.equal(context.discoverArtist,null);
 tree=build();nodes(tree,n=>n.type===PlexDiscoverView)[0].props.onOpenAlbum(album());tree=build();
 const original=nodes(tree,n=>n.type===AlbumView)[0];original.props.onSelectArtist({rating_key:'88',name:'Synthetic'});tree=build();
 assert.equal(nodes(tree,n=>n.type===AlbumView)[0].props.active,false);
 const directArtist=nodes(tree,n=>n.type===ArtistView)[0];assert.equal(directArtist.props.artist.library_id,'2');assert.equal(directArtist.props.artist.server_id,'server-a');
 directArtist.props.onBack();tree=build();assert.equal(nodes(tree,n=>n.type===AlbumView)[0].props.active,true);nodes(tree,n=>n.type===AlbumView)[0].props.onBack();assert.equal(context.discoverPages.length,0);
 // Artist → album → same artist returns to its retained parent without duplication.
 nodes(tree,n=>n.type===PlexDiscoverView)[0].props.onOpenArtist(artist);tree=build();nodes(tree,n=>n.type===ArtistView)[0].props.onSelectAlbum(album());tree=build();nodes(tree,n=>n.type===AlbumView)[0].props.onSelectArtist(artist);assert.equal(context.discoverPages.length,1);update([]);
 context.discoverScope={serverId:'server-b'};context.discoverIdentity={};discover.props.onOpenArtist(artist);sourcePage.onSelectAlbum(album());assert.equal(context.discoverArtist,null);assert.equal(context.discoverAlbum,null);
 assert.match(app,/plexDiscover\.setContext\(discoverScope\); setDiscoverPages\(\[\]\)/);
});


test('retained AlbumView restores its own scroll and focused artist link without loading or playing again',async()=>{
 const albumExports={};let loads=0,selected;
 const albumDeps={...deps,'../services/plex':{plexService:{getAlbumTracks:async()=>{loads++;return[];}}},'../services/favorites':{favoritesService:{getFavorites:async()=>[]}}};
 new Function('require','exports',compile(readFileSync(new URL('../src/components/AlbumView.tsx',import.meta.url),'utf8')))(name=>albumDeps[name],albumExports);
 const h=new Harness();h.component=albumExports.AlbumView;
 let p={album:album(),active:true,isPlaybackAvailable:true,onBack(){},onSelectArtist:a=>selected=a};let tree=h.render(p);await flush();tree=h.render(p);
 const viewport=nodes(tree,n=>n.props?.onScroll)[0];const focused=[],link={focus:options=>focused.push(options)};const node={scrollTop:0,querySelector:()=>null};viewport.props.ref.current=node;
 viewport.props.onScroll({currentTarget:{scrollTop:480}});viewport.props.onFocusCapture({target:link});
 nodes(tree,n=>n.type==='button'&&n.props.ref)[0].props.onClick();assert.equal(selected.server_id,'server-a');assert.equal(selected.library_id,'2');
 p={...p,active:false};h.render(p);node.scrollTop=0;p={...p,active:true};h.render(p);assert.equal(node.scrollTop,480);assert.deepEqual(focused,[{preventScroll:true}]);assert.equal(loads,1);h.unmount();
});
