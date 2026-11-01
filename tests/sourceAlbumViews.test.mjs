import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import ts from 'typescript';
import i18next from 'i18next';
import * as presentation from '../src/utils/albumPresentation.ts';
const read=path=>readFileSync(new URL('../src/'+path,import.meta.url),'utf8');
const ast=ts.createSourceFile('App.tsx',read('App.tsx'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
let cardSource;function visit(node){if(ts.isVariableDeclaration(node)&&node.name.getText(ast)==='PlexAlbumCard')cardSource=node.initializer.getText(ast);ts.forEachChild(node,visit)}visit(ast);assert.ok(cardSource);
function compile(source,scope){const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText,exports={};new Function('require','exports',...Object.keys(scope),code)(name=>{assert.equal(name,'react/jsx-runtime');return jsx},exports,...Object.values(scope));return exports;}
for(const locale of ['en','pt-BR'])test(`Plex row views preserve source actions and identities without per-album metadata reads or hidden artwork (${locale})`,async()=>{
 const i18n=i18next.createInstance();await i18n.init({lng:locale,resources:{[locale]:{translation:JSON.parse(read(`locales/${locale}.json`))}}});
 const deps={'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n),i18n})},'../utils/albumPresentation':presentation};
 const rowExports={};new Function('require','exports',ts.transpileModule(read('components/AlbumCatalogRow.tsx'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX}}).outputText)(id=>{assert.ok(id in deps,id);return deps[id]},rowExports);
 const calls=[],queueRequests=[];let coverReads=0;
 const PlexImage=()=>{coverReads++;return jsx.jsx('img',{alt:'Fixture cover'});};
 const {Card}=compile(`export const Card=${cardSource}`,{Disc3:icons.Disc3,Heart:icons.Heart,Play:icons.Play,memo:fn=>fn,useMemo:fn=>fn(),useQueueInsertion:()=>({menu:null,notice:null,trigger:request=>{queueRequests.push(request);return null},open:(_event,request)=>queueRequests.push(request)}),plexService:{getAlbumTracks:async key=>{calls.push(['tracks',key]);return[]}},plexQueueTrack:()=>assert.fail('no catalog track reads'),PlexImage,AlbumCatalogRow:rowExports.AlbumCatalogRow});
 const album={rating_key:'opaque:album/4',server_id:'fixture-server',library_id:'library-a',library_title:'Collection A',title:'Synthetic album',artist:'Synthetic artist',year:1998,thumb:{server_id:'fixture-server',path:'/fixture'},genres:['Jazz'],added_at:1762000000};
 const props={album,isFav:false,isPlaybackAvailable:true,onSelect:a=>calls.push(['open',a]),onPlayQuick:(_event,a)=>calls.push(['play',a]),onToggleFav:(_event,a)=>calls.push(['favorite',a]),onSelectArtist:()=>{},removeFavText:'remove',addFavText:'add',playAlbumText:'play'};
 let tree=Card({...props,layout:'details'});const html=renderToStaticMarkup(tree);assert.equal(coverReads,0);assert.doesNotMatch(html,/<img|0 tracks|0 faixas|albumViews\./);assert.ok(html.includes('Collection A'));assert.ok(html.includes('Jazz'));assert.deepEqual(calls,[]);
 const row=tree.props.children.at(-1);row.props.onOpen();row.props.onPlay({});row.props.onFavorite({});assert.deepEqual(calls.map(c=>c[0]),['open','play','favorite']);assert.ok(calls.every(c=>c[1]===album));assert.equal(queueRequests[0].scope,'album');
 await queueRequests[0].load();assert.deepEqual(calls.at(-1),['tracks','opaque:album/4']);
 renderToStaticMarkup(Card({...props,layout:'list'}));assert.equal(coverReads,1);
});
