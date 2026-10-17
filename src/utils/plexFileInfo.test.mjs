import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import i18next from 'i18next';
const read=file=>readFileSync(new URL(file,import.meta.url),'utf8');
function compile(file,deps){const ex={};new Function('require','exports',ts.transpileModule(read(file),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},ex);return ex;}
const nodes=tree=>React.isValidElement(tree)?[tree,...nodes(tree.props.children)]:Array.isArray(tree)?tree.flatMap(nodes):[];
const info={container:'FLAC',codec:'FLAC',compression:'lossless',sample_type:'integer',sample_rate:96000,bit_depth:24,dsd_level:null,channels:2,bitrate_kbps:3000};
const track={rating_key:'42',title:'Synthetic track',duration_ms:123000,media_locator:{kind:'plex',server_id:'server',part_key:'/private-part'},audio_info:info};
async function dependencies(lng='en',hooks=React){const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(read(`../locales/${lng}.json`))}},fallbackLng:false});return {i18n,deps:{react:{...hooks,default:hooks},'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n),i18n})}}};}
test('Plex info renders declared PCM, DSD and lossy metadata in both languages without private locators',async()=>{
 for(const lng of ['en','pt-BR']){
  const {i18n,deps}=await dependencies(lng),{PlexFileInfoModal:Modal}=compile('../components/PlexFileInfoModal.tsx',deps);
  for(const [audio_info,expected] of [[info,['FLAC','96000 Hz','24','3000']], [{...info,container:'DSF',codec:'DSD',sample_type:'dsd',sample_rate:5644800,dsd_level:128},['DSD128','5644800']], [{...info,codec:'MP3',sample_type:'not_applicable',compression:'lossy'},['MP3',i18n.t('fileInfo.notApplicable')]], [undefined,[i18n.t('fileInfo.unknown')]]]){
   const html=renderToStaticMarkup(React.createElement(Modal,{track:{...track,audio_info},onClose(){}}));for(const text of expected)assert.ok(html.includes(text),text);
   assert.ok(html.includes(i18n.t('fileInfo.plexProvenance')));assert.ok(html.includes(i18n.t('fileInfo.plexLimits')));assert.doesNotMatch(html,/private-part|server_id|X-Plex-Token|fileInfo\./);
   if(audio_info?.sample_type==='dsd')assert.ok(!html.includes(i18n.t('fileInfo.bitDepth')));
  }
 }
});
test('Plex info uses native focus/escape behavior and does not fetch or trigger playback',async()=>{
 let effect,shown=0,closed=0,focused=0,dismissed=0;const element={open:false,showModal(){shown++;this.open=true},close(){closed++;this.open=false}};
 const hooks={useRef:()=>({current:element}),useEffect:fn=>{effect=fn}};
 const saved={document:globalThis.document,HTMLElement:globalThis.HTMLElement};class Element{focus(){focused++}}globalThis.HTMLElement=Element;globalThis.document={activeElement:new Element()};
 try{const {deps}=await dependencies('en',hooks),{PlexFileInfoModal:Modal}=compile('../components/PlexFileInfoModal.tsx',deps);const tree=Modal({track,onClose(){dismissed++}});const cleanup=effect();assert.equal(shown,1);
  let stopped=0;tree.props.onKeyDown({stopPropagation(){stopped++}});tree.props.onCancel({preventDefault(){}});assert.equal(dismissed,1);assert.equal(stopped,1);
  nodes(tree).find(n=>n.type==='button').props.onClick();assert.equal(dismissed,2);cleanup();assert.equal(closed,1);assert.equal(focused,1);
 }finally{Object.assign(globalThis,saved)}
});
test('both Plex album hosts expose per-track info without playing or making an additional request',async()=>{
 for(const name of ['AlbumView','AlbumDetailModal']){
  let cursor=0,values=[],reads=0,plays=0;
  const hooks={useState:initial=>{const i=cursor++;if(!(i in values))values[i]=i===0?[track]:i===1?false:initial;return[values[i],v=>{values[i]=v}]},useRef:v=>({current:v}),useEffect(){},useLayoutEffect(){}};
  const {deps}=await dependencies('en',hooks);const Modal=()=>null;
  const extra={...deps, '../utils/usePlaybackAttempt':{usePlaybackAttempt:()=>({failure:null,run:action=>action({current:()=>true})})},'./PlaybackRequestNotice':{PlaybackRequestNotice:()=>null},
 '../utils/frontendDiagnostics':{logError(){}},'../services/plex':{plexService:{getAlbumTracks(){reads++;return Promise.resolve([track])}}},'../services/audio':{audioService:{playTracks(){plays++}}},'../services/favorites':{favoritesService:{}},'./PlexImage':{PlexImage:()=>null},'./PlaylistPickerModal':{PlaylistPickerModal:()=>null},'./PlexFileInfoModal':{PlexFileInfoModal:Modal}};
  const View=compile(`../components/${name}.tsx`,extra)[name];const album={rating_key:'album',title:'Album',artist:'Artist'};const props={album,onBack(){},onClose(){},isPlaybackAvailable:true};const render=()=>{cursor=0;return View(props)};
  let tree=render();const button=nodes(tree).find(n=>n.type==='button'&&'data-plex-file-info'in n.props);assert.ok(button,name);let stopped=0;button.props.onClick({stopPropagation(){stopped++}});assert.equal(stopped,1);tree=render();const modal=nodes(tree).find(n=>n.type===Modal);assert.equal(modal.props.track,track);assert.equal(plays,0);assert.equal(reads,0);
  props.album={...album};tree=render();assert.ok(nodes(tree).some(n=>n.type===Modal),'equivalent album props must keep the dialog open');
  props.album={...album,rating_key:'other'};tree=render();assert.ok(!nodes(tree).some(n=>n.type===Modal));
 }
});

test('StrictMode cleanup close event cannot dismiss the reopened native dialog',async()=>{
 let effect,dismissed=0,shown=0;const queued=[];let tree;
 const element={open:false,showModal(){this.open=true;shown++},close(){this.open=false;queued.push(()=>tree.props.onClose({currentTarget:element}))}};
 const hooks={useRef:()=>({current:element}),useEffect:fn=>{effect=fn}};
 const saved={document:globalThis.document,HTMLElement:globalThis.HTMLElement};class Element{focus(){}}globalThis.HTMLElement=Element;globalThis.document={activeElement:new Element()};
 try{
  const {deps}=await dependencies('en',hooks),{PlexFileInfoModal:Modal}=compile('../components/PlexFileInfoModal.tsx',deps);tree=Modal({track,onClose(){dismissed++}});
  const cleanup=effect();cleanup();const finalCleanup=effect();assert.equal(shown,2);queued.shift()();assert.equal(element.open,true);assert.equal(dismissed,0);
  element.close();queued.shift()();assert.equal(dismissed,1);finalCleanup();
 }finally{Object.assign(globalThis,saved)}
});
test('Plex album action order matches Local: duration, playlist then file info',()=>{
 const album=read('../components/AlbumView.tsx'),legacy=read('../components/AlbumDetailModal.tsx');
 const row=album.slice(album.indexOf('tracks.map((track, idx)'));
 assert.ok(row.indexOf('formatDuration(track.duration_ms')<row.indexOf('setPlaylistItems([toPlaylistItem(track)])'));
 assert.ok(row.indexOf('setPlaylistItems([toPlaylistItem(track)])')<row.indexOf('data-plex-file-info'));
 const popup=legacy.slice(legacy.indexOf('tracks.map((track, idx)'));assert.ok(popup.indexOf('formatDuration(track.duration_ms')<popup.indexOf('data-plex-file-info'));
});
