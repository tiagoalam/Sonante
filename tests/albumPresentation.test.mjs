import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import ts from 'typescript';
import i18next from 'i18next';
import * as model from '../src/utils/albumPresentation.ts';
import {virtualGridMetrics} from '../src/utils/virtualGrid.ts';
const compile=(name,deps)=>{const exports={};const source=readFileSync(new URL(`../src/components/${name}.tsx`,import.meta.url),'utf8');const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText;new Function('require','exports',code)(key=>{assert.ok(key in deps,key);return deps[key]},exports);return exports[name]};
const nodes=tree=>Array.isArray(tree)?tree.flatMap(nodes):!tree||typeof tree!=='object'?[]:[tree,...nodes(tree.props?.children)];
test('invalid or unavailable storage keeps the existing medium grid; each source preference is independent',()=>{
 const previous=Object.getOwnPropertyDescriptor(globalThis,'localStorage');const values=new Map();
 try{Object.defineProperty(globalThis,'localStorage',{configurable:true,value:{getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)}});
  assert.deepEqual(model.readAlbumPresentation('local'),{layout:'grid',coverSize:'medium'});
  values.set(model.albumPresentationKey('local'),'{broken');assert.deepEqual(model.readAlbumPresentation('local'),model.DEFAULT_ALBUM_PRESENTATION);
  model.saveAlbumPresentation('local',{layout:'details',coverSize:'large'});assert.deepEqual(model.readAlbumPresentation('local'),{layout:'details',coverSize:'large'});
  assert.deepEqual(model.readAlbumPresentation('plex'),model.DEFAULT_ALBUM_PRESENTATION);
  Object.defineProperty(globalThis,'localStorage',{configurable:true,get(){throw Error('unavailable')}});assert.doesNotThrow(()=>model.saveAlbumPresentation('local',{layout:'list',coverSize:'small'}));assert.deepEqual(model.readAlbumPresentation('local'),model.DEFAULT_ALBUM_PRESENTATION);
 }finally{if(previous)Object.defineProperty(globalThis,'localStorage',previous);else delete globalThis.localStorage;}
 assert.deepEqual(model.albumPresentation({layout:'invalid',coverSize:100}),model.DEFAULT_ALBUM_PRESENTATION);
});
test('all three cover sizes remain virtualized for a ten-thousand-album collection; medium retains existing geometry',()=>{
 const metrics=size=>virtualGridMetrics({itemCount:10000,containerWidth:1200,minimumColumnWidth:model.ALBUM_CARD_WIDTHS[size],gap:24,cardExtraHeight:64,scrollOffset:17000,viewportHeight:720,overscanRows:3});
 const sizes=model.ALBUM_COVER_SIZES.map(metrics);assert.ok(sizes[0].columnCount>sizes[1].columnCount&&sizes[1].columnCount>sizes[2].columnCount);
 for(const value of sizes)assert.ok((value.endRow-value.startRow)*value.columnCount<120);
 assert.equal(sizes[1].columnCount,6);assert.equal(sizes[1].cardWidth,180);
});
test('catalog uses the existing virtual grid or list, preserves album objects and forwards alphabetical jumps after layout changes',()=>{
 let cursor=0;const slots=[],effects=[];let handle;const Grid=()=>null,List=()=>null;
 const hooks={...React,forwardRef:fn=>fn,useRef:value=>slots[cursor++]??={current:value},useImperativeHandle:(_ref,create)=>{handle=create()},useLayoutEffect:fn=>effects.push(fn)};
 const Catalog=compile('AlbumCatalog',{react:hooks,'react/jsx-runtime':jsx,'./VirtualAlbumGrid':{VirtualAlbumGrid:Grid},'./VirtualList':{VirtualList:List},'../utils/albumPresentation':model});
 const items=[{id:'opaque-source/album'}],props={items,getItemKey:album=>album.id,renderItem:()=>null,scrollContainer:{},resetKey:'same'};
 const paint=(layout,coverSize='medium')=>{cursor=0;const tree=Catalog({...props,presentation:{layout,coverSize}},null);effects.splice(0).forEach(fn=>fn());return tree};
 let tree=paint('grid');assert.equal(tree.type,Grid);assert.equal(tree.props.minimumCardWidth,170);assert.equal(tree.props.items,items);
 const scroll=[];slots[0].current={scrollToIndex:index=>scroll.push(index)};tree=paint('list');assert.equal(tree.type,List);assert.equal(tree.props.rowHeight,80);assert.equal(tree.props.items[0],items[0]);handle.scrollToIndex(9);assert.deepEqual(scroll,[0,9]);
 tree=paint('details');assert.equal(tree.type,List);assert.equal(tree.props.rowHeight,96);tree=paint('grid','large');assert.equal(tree.props.minimumCardWidth,230);
});
for(const language of ['pt-BR','en'])test(`view controls and detailed rows preserve explicit actions and never render cover images (${language})`,async()=>{
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`,import.meta.url),'utf8'))}}});
 const deps={react:React,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n),i18n})},'../utils/albumPresentation':model};
 const Controls=compile('AlbumViewControls',deps),Row=compile('AlbumCatalogRow',deps),changed=[];
 let tree=Controls({value:{layout:'grid',coverSize:'medium'},onChange:value=>changed.push(value)});
 const controls=nodes(tree).filter(node=>node.type==='button');assert.equal(controls.length,3);controls[2].props.onClick();assert.deepEqual(changed,[{layout:'details',coverSize:'medium'}]);
 nodes(tree).find(node=>node.type==='select').props.onChange({target:{value:'small'}});assert.deepEqual(changed[1],{layout:'grid',coverSize:'small'});
 tree=Controls({value:changed[0],onChange(){}});assert.equal(nodes(tree).filter(node=>node.type==='select').length,0);
 const calls=[];const props={title:'Synthetic album',artist:'Synthetic artist',trackCount:12,discCount:2,year:'2001',genres:['Jazz','Fusion'],library:'Local fixture',addedAt:'2026-10-30T12:00:00Z',detailed:true,artwork:React.createElement('img',{src:'data:image/png;base64,fixture',alt:''}),favorite:false,readable:true,playable:true,onOpen:()=>calls.push('open'),onPlay:()=>calls.push('play'),onFavorite:()=>calls.push('favorite'),queueAction:React.createElement('button',null,'queue')};
 tree=Row(props);const html=renderToStaticMarkup(tree);assert.doesNotMatch(html,/<img|albumViews\.|localBrowser\./);for(const text of ['Synthetic album','Synthetic artist','Jazz','Local fixture','2001'])assert.ok(html.includes(text));assert.deepEqual(calls,[]);
 const actions=nodes(tree).filter(node=>node.type==='button');actions[0].props.onClick();actions[1].props.onClick();actions[2].props.onClick();assert.deepEqual(calls,['open','play','favorite']);
 assert.match(renderToStaticMarkup(Row({...props,detailed:false})),/<img/);
});
