import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import ts from 'typescript';
import i18next from 'i18next';
import {renderToStaticMarkup} from 'react-dom/server';
import * as directory from '../src/utils/radioDirectory.ts';
import * as intents from '../src/utils/playbackIntent.ts';
import {createLocalSourceReadQueue} from '../src/utils/localSourceReadQueue.ts';
import {queueItemIsCurrent} from '../src/utils/nowPlayingPresentation.ts';
import {currentMediaKey} from '../src/utils/playerNavigation.ts';
const read=p=>readFileSync(new URL('../src/'+p,import.meta.url),'utf8');
const compile=(path,deps,env={})=>{const exports={};new Function('require','exports',...Object.keys(env),ts.transpileModule(read(path),{fileName:path,compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText)(id=>{assert.ok(id in deps,id);return deps[id]},exports,...Object.values(env));return exports};
const safeMarkup=tree=>renderToStaticMarkup((function strip(node){if(Array.isArray(node))return node.map(strip);if(!React.isValidElement(node))return node;if(typeof node.type==='function')return React.createElement('span',null,strip(node.props.children));return React.cloneElement(node,{},strip(node.props.children));})(tree));
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
const flush=async()=>{for(let i=0;i<70;i++)await Promise.resolve()};
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const station=(id='rb:01234567-89ab-cdef-0123-456789abcdef',name='Synthetic Jazz',extra={})=>({id,name,country_code:'BR',tags:['jazz','bossa nova'],codec:'AAC',bitrate:128,favicon:null,stream_url:'https://stream.fixture.invalid/live?key=PRIVATE',manual:false,...extra});
const query=(name='',offset=0)=>({name,country:'',genre:'',offset});
const same=(a,b)=>a?.length===b?.length&&a.every((x,i)=>Object.is(x,b[i]));
let generation=0;
async function fixture(language='en') {
 let current;const calls=[];
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read('locales/'+language+'.json'))}}});
 intents.configurePlaybackIntents(async()=>String(++generation),async token=>{if(token===String(generation))generation++},async token=>token===String(generation));
 const hooks={
  useState(value){const h=current,n=h.cursor++;h.slots[n]??={value:typeof value==='function'?value():value};return[h.slots[n].value,next=>{assert.ok(!h.disposed,'no writes after unmount');h.slots[n].value=typeof next==='function'?next(h.slots[n].value):next}]},
  useRef(value){return current.slots[current.cursor++]??={current:value}},
  useMemo(fn,deps){const h=current,n=h.cursor++;if(!same(h.slots[n]?.deps,deps))h.slots[n]={deps,value:fn()};return h.slots[n].value},
  useEffect(fn,deps){const h=current,n=h.cursor++,old=h.slots[n];if(!same(old?.deps,deps))h.effects.push(()=>{old?.cleanup?.();h.slots[n]={deps,cleanup:fn()}})},
  useSyncExternalStore(subscribe,get){hooks.useEffect(()=>subscribe(()=>{}),[subscribe]);return get()},
 };
 class Host{slots=[];cursor=0;effects=[];disposed=false;constructor(render){this.render=render}paint(){current=this;this.cursor=0;this.effects=[];const tree=this.render();current=null;this.effects.splice(0).forEach(f=>f());return tree}close(){this.slots.forEach(slot=>slot?.cleanup?.());this.disposed=true}}
 const service={search:async q=>{calls.push(['search',q]);return{stations:[station()],has_more:true}},filters:async()=>{calls.push(['filters']);return{countries:[{name:'BR',stationcount:1}],genres:[{name:'jazz',stationcount:1}]}},favorites:async()=>{calls.push(['favorites']);return[]},favorite:async(id,enabled)=>{calls.push(['favorite',id,enabled]);return enabled?[station()]:[]},manual:async(name,url)=>{calls.push(['manual',name,url]);return[station('manual:'+'a'.repeat(64),name,{manual:true,stream_url:url,country_code:'',tags:[]})]},play:async(id,intent)=>{await intents.playbackIntentToken(intent);if(intent.current())calls.push(['play',id])},stop:async(id,intent)=>{await intents.playbackIntentToken(intent);if(intent.current())calls.push(['stop',id])}};
 const {RadioView}=compile('components/RadioView.tsx',{react:{...React,...hooks},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n),i18n})},'lucide-react':icons,'@tauri-apps/plugin-opener':{openUrl:async url=>calls.push(['external',url])},'../services/radio':{radioService:service},'../utils/radioDirectory':directory,'../utils/playbackIntent':intents},{document:{activeElement:{focus(){}}}});
 const f={calls,service,t:i18n.t.bind(i18n),props:{active:true,available:true,status:{state:'stop',current_media:null,playback_error:null,title:''}},Host,hooks:{...React,...hooks}};
 f.host=new Host(()=>RadioView(f.props));f.paint=()=>f.host.paint();f.paint();await flush();f.paint();return f;
}

test('radio directory is lazy, bounds one page and discards stale responses while serializing latest filters',async()=>{
 const pending=[],calls=[];const session=new directory.RadioDirectorySession(q=>{calls.push(q);const d=deferred();pending.push(d);return d.promise});assert.deepEqual(calls,[]);
 session.set(query('old'),false);assert.deepEqual(calls,[]);session.set(query('old'),true);session.set(query('intermediate'),true);session.set(query('latest'),true);assert.equal(calls.length,1);
 pending[0].resolve({stations:[station('old')],has_more:false});await flush();assert.deepEqual(calls.map(q=>q.name),['old','latest']);assert.equal(session.getSnapshot().stations.length,0);
 pending[1].resolve({stations:[station('latest'),station('latest')],has_more:true});await flush();assert.deepEqual(session.getSnapshot().stations.map(s=>s.id),['latest']);
 session.set(query('latest',40),true);pending[2].resolve({stations:[station('page2')],has_more:false});await flush();assert.deepEqual(session.getSnapshot().stations.map(s=>s.id),['page2']);
 session.set(query('hidden'),true);session.close();pending[3].resolve({stations:[station('hidden')],has_more:false});await flush();assert.deepEqual(session.getSnapshot().stations.map(s=>s.id),[]);
});
test('radio directory errors remain distinct from empty results and retry is explicit',async()=>{
 let fail=true,reads=0;const session=new directory.RadioDirectorySession(async()=>{reads++;if(fail)throw Error('PRIVATE');return{stations:[],has_more:false}});
 session.set(query(),true);await flush();assert.equal(session.getSnapshot().phase,'error');assert.equal(reads,1);fail=false;session.retry();session.retry();await flush();assert.equal(reads,2);assert.equal(session.getSnapshot().phase,'ready');assert.equal(session.getSnapshot().stations.length,0);
});
test('favorite filtering combines accent-insensitive name, country and genre without network',()=>{
 const radio=station(undefined,'Rádio São Paulo');assert.equal(directory.radioStationMatches(radio,{name:'radio sao',country:'BR',genre:'bossa',offset:0}),true);assert.equal(directory.radioStationMatches(radio,{name:'radio sao',country:'FR',genre:'bossa',offset:0}),false);
 const item={title:'Station',artist:'Station',album:'',media_locator:{kind:'radio',station_id:radio.id}};assert.equal(queueItemIsCurrent(item,1,{kind:'radio',station_id:radio.id,queue_index:1}),true);assert.equal(queueItemIsCurrent(item,1,{kind:'radio',station_id:'other',queue_index:1}),false);assert.match(currentMediaKey({kind:'radio',station_id:radio.id,queue_index:1}),/radio/);
});
for(const language of ['en','pt-BR'])test(`real radio view queries only while active, renders filters/cards and never plays on search or favorite (${language})`,async()=>{
 const f=await fixture(language);let tree=f.paint();assert.deepEqual(f.calls.map(c=>c[0]).sort(),['favorites','filters','search']);assert.equal(nodes(tree,n=>n.props?.['data-radio-station']).length,1);assert.equal(f.calls.some(c=>c[0]==='play'),false);
 const form=nodes(tree,n=>n.type==='form')[0],inputs=nodes(form,n=>n.type==='input');inputs[0].props.onChange({target:{value:'Jazz'}});tree=f.paint();nodes(nodes(tree,n=>n.type==='form')[0],n=>n.type==='input')[1].props.onChange({target:{value:'bossa'}});f.paint();assert.equal(f.calls.filter(c=>c[0]==='search').length,1);
 tree=f.paint();nodes(tree,n=>n.type==='select')[0].props.onChange({target:{value:'BR'}});tree=f.paint();nodes(tree,n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();tree=f.paint();assert.deepEqual(f.calls.filter(c=>c[0]==='search').at(-1)[1],{name:'Jazz',genre:'bossa',country:'BR',offset:0});
 const article=nodes(tree,n=>n.type==='article')[0];nodes(article,n=>n.type==='button'&&n.props['aria-pressed']!==undefined)[0].props.onClick();await flush();tree=f.paint();assert.equal(f.calls.filter(c=>c[0]==='play').length,0);assert.ok(f.calls.find(c=>c[0]==='favorite'));
 const html=safeMarkup(tree);assert.doesNotMatch(html,/radio\.(title|country|genre)|PRIVATE|stream.fixture.invalid/);f.host.close();
});
test('radio explicit Play blocks duplicates and Stop is source-bound and idempotent in the view',async()=>{
 const f=await fixture();let tree=f.paint();const slow=deferred();f.service.play=async(id,intent)=>{await intents.playbackIntentToken(intent);f.calls.push(['play',id]);await slow.promise};
 let play=nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('radio.playStation',{name:'Synthetic Jazz'}))[0];play.props.onClick();play.props.onClick();await flush();assert.equal(f.calls.filter(c=>c[0]==='play').length,1);slow.resolve();await flush();
 f.props.status={state:'play',current_media:{kind:'radio',station_id:station().id,queue_index:0},title:'Current song',playback_error:null};tree=f.paint();const stopped=deferred();f.service.stop=async(id,intent)=>{await intents.playbackIntentToken(intent);f.calls.push(['stop',id]);await stopped.promise};const stop=nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('radio.stopStation',{name:'Synthetic Jazz'}))[0];stop.props.onClick();stop.props.onClick();await flush();assert.deepEqual(f.calls.filter(c=>c[0]==='stop'),[['stop',station().id]]);stopped.resolve();await flush();f.host.close();
});
test('radio station errors are sanitized, and favorites remain usable during a directory outage',async()=>{
 const f=await fixture();f.service.search=async()=>{throw Error('PRIVATE /path')};f.service.play=async()=>{throw Error('PRIVATE /path')};let tree=f.paint();nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('radio.playStation',{name:'Synthetic Jazz'}))[0].props.onClick();await flush();tree=f.paint();assert.ok(nodes(tree,n=>n.props?.role==='alert').length);assert.doesNotMatch(safeMarkup(tree),/PRIVATE/);
 nodes(tree,n=>n.type==='button'&&n.props['aria-pressed']===false&&Array.isArray(n.props.children)&&n.props.children.includes(f.t('radio.favorites')))[0].props.onClick();tree=f.paint();const before=f.calls.filter(c=>c[0]==='search').length;f.paint();await flush();assert.equal(f.calls.filter(c=>c[0]==='search').length,before);f.host.close();
});
test('radio pending work is discarded after navigation/unmount; inactive radio triggers no network',async()=>{
 const f=await fixture();const slow=deferred();f.service.favorite=()=>slow.promise;let tree=f.paint();nodes(tree,n=>n.type==='article')[0].props.children[0].props.children[2].props.onClick();f.host.close();slow.resolve([station()]);await flush();
 const second=await fixture();const before=second.calls.length;second.props.active=false;second.paint();await flush();assert.equal(second.calls.length,before);second.host.close();
});

test('radio service requires current intent and Direct choice only for Play, while directory queries do not enter the audio control queue',async()=>{
 const calls=[],queue=createLocalSourceReadQueue();const deps={'@tauri-apps/api/core':{invoke:async(command,args)=>{calls.push([command,args]);return[]}},'./config':{ensureDirectOutputChoice:async()=>calls.push(['choice'])},'../utils/playbackIntent':intents,'../utils/localSourceReadQueue':{localSourceReadQueue:queue}};
 const {radioService}=compile('services/radio.ts',deps);const token=intents.beginPlaybackIntent();await radioService.search(query());assert.deepEqual(calls.map(c=>c[0]),['search_radio']);await radioService.play(station().id,token);assert.deepEqual(calls.slice(1).map(c=>c[0]),['choice','play_radio']);
 const stop=intents.beginPlaybackIntent();await radioService.stop(station().id,stop);assert.equal(calls.at(-1)[0],'stop_radio');const previous=intents.beginPlaybackIntent();intents.beginPlaybackIntent();await assert.rejects(radioService.play(station().id,previous));assert.equal(calls.filter(c=>c[0]==='play_radio').length,1);
});

test('manual station dialog saves only on submit, returns to favorites and never starts playback',async()=>{
 const f=await fixture();let tree=f.paint();nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(f.t('radio.addManual')))[0].props.onClick();tree=f.paint();
 const dialog=nodes(tree,n=>n.type==='dialog')[0];assert.ok(dialog);assert.equal(dialog.props['aria-labelledby'],'radio-manual-title');let inputs=nodes(dialog,n=>n.type==='input');inputs[0].props.onChange({target:{value:'My station'}});tree=f.paint();inputs=nodes(nodes(tree,n=>n.type==='dialog')[0],n=>n.type==='input');inputs[1].props.onChange({target:{value:'https://stream.fixture.invalid/manual'}});tree=f.paint();
 const submit=nodes(nodes(tree,n=>n.type==='dialog')[0],n=>n.type==='form')[0];submit.props.onSubmit({preventDefault(){}});submit.props.onSubmit({preventDefault(){}});await flush();tree=f.paint();assert.deepEqual(f.calls.filter(c=>c[0]==='manual'),[['manual','My station','https://stream.fixture.invalid/manual']]);assert.equal(nodes(tree,n=>n.type==='dialog').length,0);assert.equal(nodes(tree,n=>n.type==='article').length,1);assert.equal(f.calls.some(c=>c[0]==='play'),false);f.host.close();
});

test('radio metadata errors and filters never reuse a stale station action after a newer playback intent',async()=>{
 const f=await fixture();const slow=deferred();f.service.play=async(_id,intent)=>{await slow.promise;await intents.playbackIntentToken(intent);if(intent.current())f.calls.push(['stale-play'])};let tree=f.paint();nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('radio.playStation',{name:'Synthetic Jazz'}))[0].props.onClick();intents.beginPlaybackIntent();slow.resolve();await flush();assert.equal(f.calls.some(c=>c[0]==='stale-play'),false);assert.equal(nodes(f.paint(),n=>n.props?.role==='alert').length,0);f.host.close();
});

for (const language of ['en','pt-BR']) test(`manual radio remains visible after filtered search; edit and delete are explicit and never play (${language})`,async()=>{
 const f=await fixture(language);let tree=f.paint(),saved=[];
 f.service.manual=async(name,url)=>{f.calls.push(['manual',name,url]);saved=[station('manual:'+'a'.repeat(64),name,{manual:true,country_code:'',tags:[],stream_url:url})];return saved};
 f.service.edit=async(id,name,url)=>{f.calls.push(['edit',id,name,url]);saved=[{...saved[0],id:'manual:'+'b'.repeat(64),name,stream_url:url}];return saved};
 f.service.favorite=async(id,enabled)=>{f.calls.push(['favorite',id,enabled]);saved=saved.filter(s=>s.id!==id);return saved};
 // The prior country/name/genre filters used to silently hide the new manual station.
 const filter=()=>nodes(f.paint(),n=>n.type==='form')[0];
 nodes(filter(),n=>n.type==='input')[0].props.onChange({target:{value:'Old search'}});
 nodes(filter(),n=>n.type==='select')[0].props.onChange({target:{value:'BR'}});
 nodes(filter(),n=>n.type==='input')[1].props.onChange({target:{value:'jazz'}});
 tree=f.paint();nodes(tree,n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(f.t('radio.addManual')))[0].props.onClick();
 let dialog=()=>nodes(f.paint(),n=>n.type==='dialog')[0];
 nodes(dialog(),n=>n.type==='input')[0].props.onChange({target:{value:'My manual station'}});
 nodes(dialog(),n=>n.type==='input')[1].props.onChange({target:{value:'https://stream.fixture.invalid/one'}});
 nodes(dialog(),n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();tree=f.paint();
 assert.equal(nodes(tree,n=>n.type==='article').length,1);assert.equal(nodes(tree,n=>n.type==='dialog').length,0);
 assert.deepEqual(nodes(filter(),n=>n.type==='input').map(n=>n.props.value),['','']);assert.equal(nodes(filter(),n=>n.type==='select')[0].props.value,'');
 nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('radio.editStation',{name:'My manual station'}))[0].props.onClick();
 assert.deepEqual(nodes(dialog(),n=>n.type==='input').map(n=>n.props.value),['My manual station','https://stream.fixture.invalid/one']);
 nodes(dialog(),n=>n.type==='input')[0].props.onChange({target:{value:'Edited station'}});
 nodes(dialog(),n=>n.type==='input')[1].props.onChange({target:{value:'https://stream.fixture.invalid/two'}});
 nodes(dialog(),n=>n.type==='form')[0].props.onSubmit({preventDefault(){}});await flush();tree=f.paint();
 assert.ok(f.calls.find(c=>c[0]==='edit'));assert.equal(nodes(tree,n=>n.type==='article').length,1);
 nodes(tree,n=>n.type==='button'&&n.props['aria-label']===f.t('radio.deleteStation',{name:'Edited station'}))[0].props.onClick();await flush();tree=f.paint();
 assert.equal(nodes(tree,n=>n.type==='article').length,0);assert.equal(f.calls.some(c=>['play','stop'].includes(c[0])),false);f.host.close();
});
test('Clear filters immediately requests the unfiltered first page, and filters favorites locally',async()=>{
 const f=await fixture();let tree=f.paint();
 const form=()=>nodes(f.paint(),n=>n.type==='form')[0];
 nodes(form(),n=>n.type==='input')[0].props.onChange({target:{value:'Jazz'}});
 nodes(form(),n=>n.type==='select')[0].props.onChange({target:{value:'BR'}});
 nodes(form(),n=>n.type==='input')[1].props.onChange({target:{value:'bossa'}});
 form().props.onSubmit({preventDefault(){}});await flush();tree=f.paint();
 const clear=()=>nodes(f.paint(),n=>n.type==='button'&&Array.isArray(n.props.children)&&n.props.children.includes(f.t('radio.clearFilters')))[0];
 clear().props.onClick();f.paint();await flush();assert.deepEqual(f.calls.filter(c=>c[0]==='search').at(-1)[1],query());
 // Favorite first, switch tabs, hide it with a filter, then clear without network or playback.
 tree=f.paint();nodes(nodes(tree,n=>n.type==='article')[0],n=>n.type==='button'&&n.props['aria-pressed']!==undefined)[0].props.onClick();await flush();
 tree=f.paint();nodes(tree,n=>n.type==='button'&&n.props['aria-pressed']===false&&Array.isArray(n.props.children)&&n.props.children.includes(f.t('radio.favorites')))[0].props.onClick();f.paint();
 nodes(form(),n=>n.type==='input')[0].props.onChange({target:{value:'No match'}});assert.equal(nodes(f.paint(),n=>n.type==='article').length,0);
 const searches=f.calls.filter(c=>c[0]==='search').length;clear().props.onClick();tree=f.paint();await flush();assert.equal(nodes(tree,n=>n.type==='article').length,1);assert.equal(f.calls.filter(c=>c[0]==='search').length,searches);assert.equal(f.calls.some(c=>c[0]==='play'),false);f.host.close();
});
for (const language of ['en','pt-BR']) test(`radio card distinguishes stream failures from real output and queue diagnostics (${language})`,async()=>{
 const f=await fixture(language);
 for(const code of ['playback_error','format_unsupported','output_busy','output_unavailable','queue_uncertain','persistence_failed']) {
  f.props.status={state:'stop',current_media:{kind:'radio',station_id:station().id,queue_index:0},title:'Station',playback_error:{code}};
  const tree=f.paint(),alert=nodes(nodes(tree,n=>n.type==='article')[0],n=>n.props?.role==='alert')[0];
  const key=code==='playback_error'?'radio.playError':code==='format_unsupported'?'radio.directError':`player.playbackError.${code}`;
  assert.equal(alert.props.children,f.t(key));assert.ok(!safeMarkup(tree).includes(f.t('player.playbackError.format_unsupported')));
 }
 f.props.status={...f.props.status,state:'play',playback_error:null};assert.equal(nodes(f.paint(),n=>n.props?.role==='alert').length,0);
 for(const code of ['output_busy','output_unavailable']) {f.props.status={...f.props.status,playback_error:{code}};assert.equal(nodes(f.paint(),n=>n.props?.role==='alert').length,0)}
 f.host.close();
});
