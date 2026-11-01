import * as musicSources from '../src/utils/musicSources.ts';
import {navidromeQueueTrack} from '../src/utils/navidromeTracks.ts';
import * as genreFamilies from '../src/utils/localGenreFamilies.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
const read=file=>readFileSync(new URL('../'+file,import.meta.url),'utf8');
function compile(file,deps={}){const exports={};new Function('require','exports',ts.transpileModule(read('src/'+file),{fileName:file,compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);return exports;}
const helpers=compile('utils/smartPlaylists.ts',{'./localGenreFamilies':genreFamilies});
const {createLocalSourceReadQueue}=compile('utils/localSourceReadQueue.ts');
const service=compile('services/smartPlaylists.ts',{'@tauri-apps/api/core':{invoke(){}},'../utils/localSourceReadQueue':{localSourceReadQueue:work=>work()},'../utils/smartPlaylistPreview':{assembleSmartPreview(){}},'./audio':{audioService:{}},'./plex':{plexService:{}},'./navidrome':{navidromeService:{}},'../utils/navidromeTracks':{navidromeQueueTrack}});
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const albums=[{id:'a',title:'A',artist:'Artist',genres:['Jazz','Reggae'],year:1965,added_at:1},{id:'b',title:'B',artist:'artist',genres:['jazz'],year:null,added_at:null},{id:'c',title:'C',artist:'Artist',genres:['Reggae'],year:null,added_at:null}];
const track={title:'Track',artist:'Artist',album:'Album',media_locator:{kind:'local',uri:'local/1.flac'},duration:120};
const preview={tracks:[track],matched_albums:1,limit_reached:false};
const item={id:'smart-1',name:'Jazz',created_at:1,updated_at:1,rules:helpers.defaultSmartRules()};
function nodes(tree,predicate,result=[]){if(!tree||typeof tree!=='object')return result;if(Array.isArray(tree)){tree.forEach(n=>nodes(n,predicate,result));return result;}if(predicate(tree))result.push(tree);nodes(tree.props?.children,predicate,result);return result;}
function text(tree){if(tree==null||typeof tree==='boolean')return '';if(typeof tree==='string'||typeof tree==='number')return String(tree);if(Array.isArray(tree))return tree.map(text).join(' ');return text(tree.props?.children);}
async function fixture(options={}){
 const language=options.language??'en';const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read(`src/locales/${language}.json`))}},interpolation:{escapeValue:false}});const t=i18n.t.bind(i18n);
 let cursor=0,disposed=false,closed=0;const slots=[],effects=[],calls=[],saved=[];const listeners=new Map();const old={document:globalThis.document,HTMLElement:globalThis.HTMLElement};globalThis.document={activeElement:null,addEventListener:(key,fn)=>listeners.set(key,fn),removeEventListener:(key,fn)=>{if(listeners.get(key)===fn)listeners.delete(key)}};globalThis.HTMLElement=class{};
 const react={...React,default:React,useState(initial){const n=cursor++;if(!(n in slots))slots[n]=typeof initial==='function'?initial():initial;return[slots[n],value=>{assert.equal(disposed,false,'no setState after unmount');slots[n]=typeof value==='function'?value(slots[n]):value}]},useRef(initial){return slots[cursor++]??={current:initial}},useMemo(fn){cursor++;return fn()},useEffect(fn,deps){const n=cursor++,prior=slots[n];if(!prior||deps.some((v,i)=>!Object.is(v,prior.deps[i])))effects.push(()=>{prior?.cleanup?.();slots[n]={deps,setup:fn,cleanup:fn()};})}};
 const native={list:async()=>{calls.push(['list']);return[item]},sources:async()=>options.sources??({local:true,plex_server_id:'server-a'}),catalog:async(source,current)=>{calls.push(['catalog',source]);return options.catalog?options.catalog(source,current):albums},preview:async rules=>{calls.push(['preview',rules]);return options.preview?options.preview(rules):preview},save:async(id,name,rules)=>{calls.push(['save',id,name,rules]);return[item]},delete:async id=>{calls.push(['delete',id]);return[]}};
 const dependencies={'../utils/musicSources':musicSources,react,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t})},'lucide-react':icons,'../services/smartPlaylists':{smartPlaylistsService:native,smartError:service.smartError},'../services/playlists':{playlistsService:{createWithItems:async(...args)=>{calls.push(['snapshot',...args]);if(options.snapshot)return options.snapshot(...args);return {id:'manual-1',name:args[0],items:args[1]};}},isDuplicatePlaylistNameError:e=>String(e).includes('playlist_name_duplicate')},'../services/plex':{plexService:{getLibraries:async()=>[{key:'7',title:'Music'}]}},'../services/audio':{audioService:{playTracks:async(...args)=>{calls.push(['play',...args]);if(options.play)return options.play(...args)}}},'../utils/playbackIntent':{beginPlaybackIntent:()=>({current:()=>true,confirmCurrent:async()=>true,cancel(){calls.push(['cancelIntent'])}}),isPlaybackSuperseded:e=>e==='PLAYBACK_SUPERSEDED',playbackFailure:()=> 'failed'},'../utils/smartPlaylists':helpers,'../utils/localGenreFamilies':genreFamilies};
 const component=compile('components/SmartPlaylists.tsx',dependencies);const props={item:options.item??null,sources:options.sources??{local:true,plex_server_id:'server-a'},isPlaybackAvailable:true,onClose(){closed++},onSaved:list=>saved.push(list),onSelectionSaved:playlist=>calls.push(['snapshotCreated',playlist])};
 const render=()=>{cursor=0;const tree=options.panel?component.SmartPlaylists({config:options.config,isPlaybackAvailable:true,onCount:options.onCount??onCount}):component.SmartPlaylistDialog(props);effects.splice(0).forEach(fn=>fn());return tree};function onCount(){}
 const button=(tree,key)=>{const matches=nodes(tree,n=>n.type==='button'&&text(n).trim()===t(key));assert.equal(matches.length,1,key);return matches[0]};
 const field=(tree,key)=>{const label=nodes(tree,n=>n.type==='label'&&text(n.props.children?.[0])===t(key))[0];assert.ok(label,key);return nodes(label,n=>n.type==='input'||n.type==='select')[0]};
 return{render,calls,saved,button,field,t,replayEffects(){slots.forEach(slot=>slot?.cleanup?.());slots.forEach(slot=>{if(slot?.setup)slot.cleanup=slot.setup();});},get closed(){return closed},async settle(){let tree;for(let n=0;n<4;n++){tree=render();await flush();}return tree},dispose(){slots.forEach(slot=>slot?.cleanup?.());disposed=true;Object.assign(globalThis,old)}};
}
test('templates keep a single selected source, apply explicit rules and use deduplicated catalog options',()=>{
 const source={kind:'plex',server_id:'server-a',library_id:'7'};for(const template of ['custom','recent','favorites','genre','decade'])assert.deepEqual(helpers.smartTemplate(template,helpers.defaultSmartRules(source),albums).source,source);
 assert.deepEqual(helpers.smartOptions(albums,'genres'),['Jazz','Reggae']);assert.deepEqual(helpers.smartOptions(albums,'artist'),['Artist']);assert.equal(helpers.smartTemplate('recent',helpers.defaultSmartRules(),albums).added_days,30);assert.equal(helpers.smartTemplate('favorites',helpers.defaultSmartRules(),albums).favorites_only,true);
 for(const bad of [{limit:0},{limit:501},{year_from:1970,year_to:1960},{added_days:0},{source:{kind:'plex',server_id:'server-a',library_id:''}}])assert.equal(helpers.smartRulesValid({...helpers.defaultSmartRules(),...bad}),false);
});
test('opening Playlists lists definitions only, without reading any catalog or changing playback',async()=>{
 const f=await fixture({panel:true});try{const tree=await f.settle();assert.ok(text(tree).includes('Jazz'));assert.deepEqual(f.calls,[['list']]);assert.equal(f.button(tree,'smart.create').props.disabled,false);}finally{f.dispose()}
});
for(const language of ['en','pt-BR'])test(`creation offers real catalog values and requires a fresh explicit preview before saving (${language})`,async()=>{
 const f=await fixture({language});try{let tree=await f.settle();assert.equal(f.calls.filter(c=>c[0]==='preview').length,0);assert.equal(f.button(tree,'smart.save').props.disabled,true);assert.ok(text(f.field(tree,'smart.genre')).includes('Reggae'));f.field(tree,'smart.name').props.onChange({target:{value:'Saved rules'}});tree=f.render();await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();assert.equal(f.button(tree,'smart.save').props.disabled,false);f.field(tree,'smart.yearFrom').props.onChange({target:{value:'1960'}});tree=f.render();assert.equal(f.button(tree,'smart.save').props.disabled,true);assert.equal(f.calls.filter(c=>c[0]==='play'||c[0]==='save').length,0);assert.ok(!text(tree).includes('smart.'));}finally{f.dispose()}
});
test('an outdated preview after rule changes cannot enable save or playback',async()=>{
 const pending=deferred();const f=await fixture({preview:()=>pending.promise});try{let tree=await f.settle();f.button(tree,'smart.preview').props.onClick();tree=f.render();f.field(tree,'smart.yearFrom').props.onChange({target:{value:'1970'}});tree=f.render();pending.resolve(preview);await flush();tree=f.render();assert.equal(f.button(tree,'smart.save').props.disabled,true);assert.equal(f.button(tree,'playlists.play').props.disabled,true);assert.equal(nodes(tree,n=>n.type==='p'&&text(n)===track.title).length,0);}finally{f.dispose()}
});
test('saved rules evaluate again when opened and playback uses exactly the visible source-typed preview',async()=>{
 const f=await fixture({item});try{let tree=await f.settle();assert.equal(f.calls.filter(c=>c[0]==='preview').length,1);assert.equal(f.calls.filter(c=>c[0]==='catalog').length,0);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);await f.button(tree,'playlists.play').props.onClick();await flush();const play=f.calls.find(c=>c[0]==='play');assert.deepEqual(play[1],[track]);assert.equal(play[2],0);}finally{f.dispose()}
});
test('read failure never becomes an empty successful selection or changes an existing playlist',async()=>{
 const f=await fixture({item,preview:async()=>{throw 'SMART_PLAYLIST_SOURCE'}});try{const tree=await f.settle();assert.ok(text(tree).includes(f.t('smart.errors.source')));assert.equal(f.button(tree,'playlists.play').props.disabled,true);assert.equal(f.calls.filter(c=>c[0]==='save'||c[0]==='delete'||c[0]==='play').length,0);}finally{f.dispose()}
});
test('closed dialogs fence late preview/catalog responses without saving or starting playback',async()=>{
 const pending=deferred();const f=await fixture({item,preview:()=>pending.promise});await f.settle();f.dispose();pending.resolve(preview);await flush();assert.equal(f.calls.filter(c=>c[0]==='play'||c[0]==='save').length,0);
});
test('saving persists rules only after a current preview and does not start playback',async()=>{
 const f=await fixture();try{let tree=await f.settle();f.field(tree,'smart.name').props.onChange({target:{value:'My Jazz'}});tree=f.render();f.field(tree,'smart.genre').props.onChange({target:{value:'jazz'}});tree=f.render();await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();await f.button(tree,'smart.save').props.onClick();await flush();const saved=f.calls.find(c=>c[0]==='save');assert.equal(saved[1],null);assert.equal(saved[2],'My Jazz');assert.equal(saved[3].genre,null);assert.equal(saved[3].genre_family,'jazz');assert.deepEqual(saved[3].source,{kind:'local'});assert.ok(!('tracks' in saved[3]));assert.equal(f.saved.length,1);assert.equal(f.calls.filter(c=>c[0]==='play').length,0);}finally{f.dispose()}
});
test('choosing Plex binds its library and resets Local filters before preview',async()=>{
 const f=await fixture();try{let tree=await f.settle();f.field(tree,'smart.genre').props.onChange({target:{value:'jazz'}});tree=f.render();f.field(tree,'smart.source').props.onChange({target:{value:'plex'}});tree=await f.settle();assert.equal(f.field(tree,'smart.genre').props.value,'');assert.equal(f.field(tree,'smart.library').props.value,'7');await f.button(tree,'smart.preview').props.onClick();await flush();assert.deepEqual(f.calls.filter(c=>c[0]==='preview').at(-1)[1].source,{kind:'plex',server_id:'server-a',library_id:'7'});}finally{f.dispose()}
});
test('repeated Play clicks share one pending request and closing cancels its owned preparation',async()=>{
 const pending=deferred();const f=await fixture({item,play:()=>pending.promise});let tree=await f.settle();f.button(tree,'playlists.play').props.onClick();f.button(tree,'playlists.play').props.onClick();assert.equal(f.calls.filter(c=>c[0]==='play').length,1);f.dispose();pending.resolve();await flush();assert.equal(f.calls.filter(c=>c[0]==='cancelIntent').length,1);
});
test('a failed refresh invalidates the earlier preview instead of enabling play or save on stale data',async()=>{
 let calls=0;const f=await fixture({item,preview:async()=>{if(calls++===0)return preview;throw 'SMART_PLAYLIST_READ'}});try{let tree=await f.settle();assert.equal(f.button(tree,'playlists.play').props.disabled,false);await f.button(tree,'smart.refresh').props.onClick();await flush();tree=f.render();assert.equal(f.button(tree,'playlists.play').props.disabled,true);assert.ok(text(tree).includes(f.t('smart.errors.read')));}finally{f.dispose()}
});
test('a saved Plex rule never changes servers silently; editing offers an explicit rebind and fresh preview',async()=>{
 const old={...item,rules:{...item.rules,source:{kind:'plex',server_id:'old-server',library_id:'7'}}};const f=await fixture({item:old});try{let tree=await f.settle();await f.button(tree,'smart.edit').props.onClick();tree=await f.settle();assert.deepEqual(f.calls.find(c=>c[0]==='preview')[1].source,old.rules.source);f.button(tree,'smart.useCurrentServer').props.onClick();tree=await f.settle();assert.equal(f.button(tree,'smart.save').props.disabled,true);await f.button(tree,'smart.preview').props.onClick();await flush();assert.equal(f.calls.filter(c=>c[0]==='preview').at(-1)[1].source.server_id,'server-a');}finally{f.dispose()}
});
test('actual editor survives the DEV StrictMode effect replay on its first opening without a retry',async()=>{
 const queue=createLocalSourceReadQueue();let reads=0;const f=await fixture({catalog:(source,current)=>queue(async()=>{if(!current())throw 'SMART_PLAYLIST_CANCELLED';reads++;return albums;})});try{f.render();f.replayEffects();const tree=await f.settle();assert.equal(reads,1,'obsolete opening must be discarded before native read');assert.equal(nodes(tree,n=>n.props?.role==='alert').length,0);assert.ok(text(f.field(tree,'smart.genre')).includes('Reggae'));assert.equal(f.calls.filter(c=>c[0]==='preview').length,0);}finally{f.dispose()}
});

for (const language of ['en','pt-BR']) test(`Local families replace the raw list; exact tags remain an explicit choice (${language})`,async()=>{
 const f=await fixture({language,catalog:async()=>[
  {...albums[0],genres:['Bossanova, Samba, MPB']},{...albums[1],genres:['Brazilian Music;Rap/Hip Hop']},
  {...albums[2],genres:['16']}
 ]});try{
  let tree=await f.settle();const field=f.field(tree,'smart.genre');
  assert.ok(text(field).includes(f.t('localGenreFamilies.brazilian')));
  assert.ok(text(field).includes(f.t('localGenreFamilies.other')));
  assert.ok(!text(field).includes('Bossanova'));
  field.props.onChange({target:{value:'brazilian'}});tree=f.render();
  await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();
  assert.equal(f.calls.find(c=>c[0]==='preview')[1].genre_family,'brazilian');
  f.button(tree,'smart.useOriginalTag').props.onClick();tree=f.render();
  assert.ok(text(f.field(tree,'smart.genre')).includes('Bossanova, Samba, MPB'));
  assert.equal(f.button(tree,'smart.save').props.disabled,true,'mode changes invalidate the old preview');
  f.field(tree,'smart.genre').props.onChange({target:{value:'Bossanova, Samba, MPB'}});tree=f.render();
  await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();
  const rules=f.calls.filter(c=>c[0]==='preview').at(-1)[1];assert.equal(rules.genre,'Bossanova, Samba, MPB');assert.equal(rules.genre_family,null);
 }finally{f.dispose()}
});
test('legacy exact Local genre opens as an original tag and missing track mode retains album sequence',async()=>{
 const old={...item,rules:{...item.rules,genre:'Jazz'}};delete old.rules.genre_family;delete old.rules.track_mode;
 const f=await fixture({item:old});try{let tree=await f.settle();f.button(tree,'smart.edit').props.onClick();tree=await f.settle();assert.equal(f.field(tree,'smart.genre').props.value,'Jazz');assert.equal(f.field(tree,'smart.trackMode').props.value,'album');assert.ok(f.button(tree,'smart.useGenreFamily'));assert.equal(f.calls.filter(c=>c[0]==='save').length,0);}finally{f.dispose()}
});
test('Plex keeps original genres and has no Local family toggle',async()=>{
 const f=await fixture();try{let tree=await f.settle();f.field(tree,'smart.source').props.onChange({target:{value:'plex'}});tree=await f.settle();assert.ok(text(f.field(tree,'smart.genre')).includes('Reggae'));assert.equal(nodes(tree,n=>n.type==='button'&&text(n)===f.t('smart.useOriginalTag')).length,0);f.field(tree,'smart.genre').props.onChange({target:{value:'Jazz'}});tree=f.render();await f.button(tree,'smart.preview').props.onClick();await flush();const rules=f.calls.find(c=>c[0]==='preview')[1];assert.equal(rules.genre,'Jazz');assert.equal(rules.genre_family,null);}finally{f.dispose()}
});
test('track selection mode defaults to varied and changing it invalidates preview before rule save',async()=>{
 const f=await fixture();try{let tree=await f.settle();assert.equal(f.field(tree,'smart.trackMode').props.value,'varied');await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();f.field(tree,'smart.trackMode').props.onChange({target:{value:'album'}});tree=f.render();assert.equal(f.button(tree,'smart.save').props.disabled,true);await f.button(tree,'smart.preview').props.onClick();await flush();assert.equal(f.calls.filter(c=>c[0]==='preview').at(-1)[1].track_mode,'album');}finally{f.dispose()}
});
test('saving a generated selection stores exactly the visible tracks, separately from rules, without playback or regeneration',async()=>{
 const second={...track,title:'Second',duration:0,media_locator:{kind:'local',uri:'local/2.flac'}};
 const f=await fixture({item,preview:async()=>({...preview,tracks:[second,track]})});try{let tree=await f.settle();f.button(tree,'smart.saveSelection').props.onClick();tree=f.render();f.field(tree,'smart.selectionName').props.onChange({target:{value:'My fixed selection'}});tree=f.render();const form=nodes(tree,n=>n.type==='form')[0];await form.props.onSubmit({preventDefault(){}});await flush();
  const call=f.calls.find(c=>c[0]==='snapshot');assert.equal(call[1],'My fixed selection');assert.deepEqual(call[2],[second,track].map(t=>({media_locator:t.media_locator,metadata:{title:t.title,artist:t.artist,album:t.album,duration:t.duration}})));
  assert.equal(f.calls.filter(c=>c[0]==='preview').length,1);assert.equal(f.calls.filter(c=>c[0]==='save'||c[0]==='play').length,0);assert.equal(f.calls.filter(c=>c[0]==='snapshotCreated').length,1);assert.equal(f.closed,1);
 }finally{f.dispose()}
});
test('duplicate snapshot name preserves preview and can be corrected without overwriting or saving smart rules',async()=>{
 let attempts=0;const f=await fixture({item,snapshot:async(name,items)=>{if(attempts++===0)throw 'playlist_name_duplicate';return{id:'manual',name,items}}});try{let tree=await f.settle();f.button(tree,'smart.saveSelection').props.onClick();tree=f.render();await nodes(tree,n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();tree=f.render();assert.ok(text(tree).includes(f.t('smart.errors.selectionDuplicate')));assert.equal(f.closed,0);assert.equal(f.button(tree,'playlists.play').props.disabled,false);f.field(tree,'smart.selectionName').props.onChange({target:{value:'Another name'}});tree=f.render();await nodes(tree,n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();assert.equal(f.calls.filter(c=>c[0]==='snapshot').length,2);assert.equal(f.calls.filter(c=>c[0]==='preview').length,1);assert.equal(f.calls.filter(c=>c[0]==='save'||c[0]==='play').length,0);}finally{f.dispose()}
});
test('a changed rule cannot save the previous selection and an in-flight snapshot cannot be submitted twice',async()=>{
 const pending=deferred();const f=await fixture({snapshot:()=>pending.promise});try{let tree=await f.settle();await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();f.button(tree,'smart.saveSelection').props.onClick();tree=f.render();f.field(tree,'smart.yearFrom').props.onChange({target:{value:'1970'}});tree=f.render();assert.equal(nodes(tree,n=>n.type==='form').length,0);assert.equal(f.button(tree,'smart.saveSelection').props.disabled,true);
  await f.button(tree,'smart.preview').props.onClick();await flush();tree=f.render();f.button(tree,'smart.saveSelection').props.onClick();tree=f.render();f.field(tree,'smart.selectionName').props.onChange({target:{value:'Fixed'}});tree=f.render();const form=nodes(tree,n=>n.type==='form')[0];const one=form.props.onSubmit({preventDefault(){}});form.props.onSubmit({preventDefault(){}});assert.equal(f.calls.filter(c=>c[0]==='snapshot').length,1);pending.resolve({id:'manual',name:'Fixed',items:[]});await one;await flush();
 }finally{f.dispose()}
});

test('Navidrome-only sources offer creation and rules without reading disabled Local or Plex catalogs',async()=>{
 const sources={local:false,plex_server_id:null,navidrome_account_id:'a'.repeat(64)};
 const f=await fixture({sources});try{const tree=await f.settle();assert.equal(f.field(tree,'smart.source').props.value,'navidrome');assert.equal(f.calls.filter(c=>c[0]==='catalog').length,1);assert.deepEqual(f.calls.find(c=>c[0]==='catalog')[1],{kind:'navidrome',account_id:'a'.repeat(64)});assert.ok(helpers.smartRulesValid({...helpers.defaultSmartRules({kind:'navidrome',account_id:'a'.repeat(64)})}));}finally{f.dispose();}
 const panel=await fixture({panel:true,sources,config:{sources:{local:false,plex:false,navidrome:true}}});try{const tree=await panel.settle();assert.equal(panel.button(tree,'smart.create').props.disabled,false);assert.ok(!text(tree).includes('Jazz'));assert.equal(panel.calls.filter(c=>c[0]==='catalog').length,0);}finally{panel.dispose();}
});
