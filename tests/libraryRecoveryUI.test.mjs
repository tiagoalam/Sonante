import * as recoveryTypes from "../src/types/libraryRecovery.ts";
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import {renderToStaticMarkup} from 'react-dom/server';
import i18next from 'i18next';
function compile(path,deps){const source=readFileSync(new URL(path,import.meta.url),'utf8');const code=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const exports={};new Function('require','exports',code)(id=>{if(id==='../types/libraryRecovery')return recoveryTypes;assert.ok(id in deps,id);return deps[id]},exports);return exports;}
const recoveryExports=compile('../src/services/libraryRecovery.ts',{'@tauri-apps/api/core':{invoke:()=>assert.fail('pure diagnostic helper must not invoke IPC')}});
const resources=Object.fromEntries(['pt-BR','en'].map(lang=>[lang,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${lang}.json`,import.meta.url),'utf8'))}]));
const reasons=['intent_valid','intent_invalid','orphan_staging','config_missing','config_invalid','config_divergent','mirror_missing','mirror_invalid','generation_divergent','database_missing','database_unproven','ledger_missing','ledger_divergent','unsafe_entry','filesystem_changed','ownership_unknown','no_pending_marker'];
const observation={analysis_id:7,reasons,intent_version:2,queue_backup_version:2,config_matches:'previous',recovery_available:false};
function deferred(){let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject};}
function nodes(n){return React.isValidElement(n)?[n,...nodes(n.props.children)]:Array.isArray(n)?n.flatMap(nodes):[];}
async function host(language='pt-BR'){
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources,fallbackLng:false});let cursor=0;const slots=[],effects=[],calls=[],pending=[],exports=[],executions=[],reopenings=[];
 const hooks={useState(value){const i=cursor++;if(!(i in slots))slots[i]=value;return[slots[i],next=>slots[i]=typeof next==='function'?next(slots[i]):next]},useRef(value){const i=cursor++;if(!(i in slots))slots[i]={current:value};return slots[i]},useEffect(fn){const i=cursor++;if(!(i in slots)){slots[i]=true;effects.push(fn)}}};
 const service={diagnostic:async()=>({kind:'unknown'}),execute(analysisId,planId){calls.push(['execute',analysisId,planId]);const d=deferred();d.progress=arguments[2];executions.push(d);return d.promise},reopen(planId){calls.push(['reopen',planId]);const d=deferred();reopenings.push(d);return d.promise},analyze(id){calls.push(['analyze',id]);const d=deferred();pending.push(d);return d.promise},cancel(id){calls.push(['cancel',id]);return Promise.resolve()},exportReport(id){calls.push(['export',id]);const d=deferred();exports.push(d);return d.promise}};
 const {LibraryRecoveryPanel}=compile('../src/components/LibraryRecoveryPanel.tsx',{react:hooks,'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../services/libraryRecovery':{...recoveryExports,libraryRecoveryService:service},'../utils/frontendDiagnostics':{logError:(...args)=>calls.push(['log',...args])}});
 const draw=()=>{cursor=0;return LibraryRecoveryPanel()};draw();const cleanups=effects.map(fn=>fn());
 const click=key=>{const button=nodes(draw()).find(n=>n.type==='button'&&n.props.children===i18n.t('libraryRecovery.'+key));assert.ok(button,key);assert.ok(!button.props.disabled,key);return button.props.onClick()};
 return{i18n,draw,click,pending,exports,executions,reopenings,calls,settle:async()=>{for(let i=0;i<8;i++)await Promise.resolve()},cleanup:()=>cleanups.forEach(fn=>fn?.()),text:()=>renderToStaticMarkup(draw())};
}
for(const language of ['pt-BR','en'])test(`real recovery panel localizes reasons and exports in ${language}`,async()=>{
 const h=await host(language);h.click('analyze');assert.equal(h.calls.length,1);assert.ok(h.text().includes(h.i18n.t('libraryRecovery.analyzing')));h.pending[0].resolve(observation);await h.settle();const markup=h.text();
 for(const reason of reasons)assert.ok(markup.includes(h.i18n.t(`libraryRecovery.reasons.${reason}`)),reason);
 assert.ok(markup.includes(h.i18n.t('libraryRecovery.unavailable')));assert.match(markup,/<details/);assert.doesNotMatch(markup,/libraryRecovery\.|\/home\/|X-Plex-Token/);
 assert.equal(nodes(h.draw()).filter(n=>n.type==='button').length,2,'only analyze and export; no repair or bootstrap');h.click('export');h.exports[0].resolve(true);await h.settle();assert.deepEqual(h.calls[1],['export',7]);assert.ok(h.text().includes(h.i18n.t('libraryRecovery.exported')));h.cleanup();
});
test('cancel invalidates a delayed result and another request wins',async()=>{
 const h=await host();h.click('analyze');const firstId=h.calls[0][1];h.click('cancel');await h.settle();assert.deepEqual(h.calls[1],['cancel',firstId]);h.click('analyze');assert.notEqual(h.calls[2][1],firstId);
 h.pending[0].resolve(observation);await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.analyzing')));h.pending[1].resolve({...observation,analysis_id:8});await h.settle();h.click('export');assert.deepEqual(h.calls[3],['export',8]);h.exports[0].resolve(false);await h.settle();assert.ok(!h.text().includes(h.i18n.t('libraryRecovery.exported')));h.cleanup();
});
test('errors never render raw diagnostics and unmount invalidates pending work',async()=>{
 const h=await host();h.click('analyze');h.pending[0].reject(new Error('/home/private?X-Plex-Token=secret'));await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.failed')));assert.doesNotMatch(h.text(),/secret|\/home\//);
 h.click('analyze');h.pending[1].resolve(observation);await h.settle();h.click('export');h.exports[0].reject(new Error('private path'));await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.exportFailed')));h.click('again');const beforeUnmount=h.text();h.cleanup();h.pending[2].resolve({...observation,analysis_id:999,reasons:['intent_invalid']});await h.settle();assert.equal(h.text(),beforeUnmount,'late result cannot replace the retained prior observation');
});
test('real App gates diagnosis on bootstrap failure and service uses dedicated IPC',()=>{
 const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');assert.match(app,/if \(!config \|\| mediaSource === null\) \{/);assert.match(app,/bootstrapError && <LibraryRecoveryPanel \/>/);
 const calls=[];const{libraryRecoveryService:s}=compile('../src/services/libraryRecovery.ts',{'@tauri-apps/api/core':{invoke:(...args)=>{calls.push(args);return Promise.resolve()}}});s.analyze('request-id');s.cancel('request-id');s.exportReport(9);assert.deepEqual(calls,[['analyze_library_recovery',{requestId:'request-id'}],['cancel_library_recovery_analysis',{requestId:'request-id'}],['export_library_recovery_report',{analysisId:9}]]);
});

const executable={...observation,recovery_available:true,plan:{id:'backend-plan',choice:'restore_previous',previous_tracks:2,backup_bytes:1024,preserves_current_references:true,resumes_interrupted:false}};
for(const language of ['pt-BR','en'])test(`explicit execution and reopen use only the backend plan in ${language}`,async()=>{
 const h=await host(language);h.click('analyze');h.pending[0].resolve(executable);await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.planReady')));assert.ok(h.text().includes(h.i18n.t('libraryRecovery.noMusicBackup')));
 const action=nodes(h.draw()).find(n=>n.type==='button'&&n.props.children===h.i18n.t('libraryRecovery.execute'));action.props.onClick();action.props.onClick();assert.deepEqual(h.calls.filter(c=>c[0]==='execute'),[['execute',7,'backend-plan']]);
 assert.equal(h.calls.filter(c=>c[0]==='reopen').length,0);h.executions[0].resolve({completed:true,backup_retained:true,requires_reopen:true});await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.completed')));assert.equal(h.calls.filter(c=>c[0]==='reopen').length,0,'successful recovery does not autoplay or bootstrap');
 h.click('reopen');assert.deepEqual(h.calls.at(-1),['reopen','backend-plan']);h.reopenings[0].resolve();await h.settle();h.cleanup();
});
test('cancel or uncertain execution cannot publish success or reopen',async()=>{
 for(const cancel of [false,true]){
  const h=await host();h.click('analyze');h.pending[0].resolve(executable);await h.settle();h.click('execute');if(cancel){h.click('cancelExecution');await h.settle();}
  h.executions[0].resolve({completed:cancel,backup_retained:true,requires_reopen:true});await h.settle();assert.ok(!h.text().includes(h.i18n.t('libraryRecovery.completed')));assert.equal(h.calls.filter(c=>c[0]==='reopen').length,0);h.cleanup();
 }
});
test('legacy or absent evidence never exposes Execute and errors stay sanitized',async()=>{
 const h=await host();h.click('analyze');h.pending[0].resolve({...observation,recovery_available:true});await h.settle();assert.ok(!nodes(h.draw()).some(n=>n.type==='button'&&n.props.children===h.i18n.t('libraryRecovery.execute')));
 h.click('again');h.pending[1].resolve(executable);await h.settle();h.click('execute');h.executions[0].reject(new Error('/home/private X-Plex-Token=secret'));await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.executionFailed')));assert.doesNotMatch(h.text(),/secret|private/);h.cleanup();
});

test('recovery keeps the impact and explicit action visible while technical evidence stays collapsed in both languages',async()=>{
 function visible(node){
  if(Array.isArray(node))return node.map(visible);
  if(!React.isValidElement(node))return node;
  if(node.type==='details')return null;
  return React.cloneElement(node,{},visible(node.props.children));
 }
 for(const language of ['pt-BR','en']){
  const h=await host(language);
  assert.equal(h.calls.length,0,'opening recovery never runs analysis or execution automatically');
  h.click('analyze');h.pending[0].resolve(executable);await h.settle();
  const tree=h.draw();const main=renderToStaticMarkup(visible(tree));
  assert.ok(main.includes(h.i18n.t('libraryRecovery.summaryImpact')));
  assert.ok(main.includes(h.i18n.t('libraryRecovery.execute')));
  assert.ok(!main.includes(h.i18n.t('libraryRecovery.impact')));
  for(const reason of executable.reasons)assert.ok(!main.includes(h.i18n.t(`libraryRecovery.reasons.${reason}`)));
  const details=nodes(tree).filter(node=>node.type==='details');
  assert.ok(details.length>0);assert.ok(details.every(node=>!node.props.open),'details are closed by default');
  assert.equal(h.calls.filter(call=>call[0]==='execute').length,0,'displaying a plan cannot execute recovery');
  h.click('execute');assert.equal(h.calls.filter(call=>call[0]==='execute').length,1);
  h.executions[0].resolve({completed:true,backup_retained:true,requires_reopen:true});await h.settle();
  assert.ok(h.text().includes(h.i18n.t('libraryRecovery.completed')));h.cleanup();
 }
});

for (const language of ['pt-BR', 'en']) test(`pending recovery reports progress without claiming failure in ${language}`, async () => {
 const h = await host(language);
 h.click('analyze'); h.pending[0].resolve(executable); await h.settle(); h.click('execute');
 const pendingMarkup = h.text();
 assert.ok(pendingMarkup.includes(h.i18n.t('libraryRecovery.progress.checking')));
 assert.ok(!pendingMarkup.includes(h.i18n.t('libraryRecovery.unavailable')), 'consumed offer is not a failed analysis');
 assert.ok(!pendingMarkup.includes(h.i18n.t('libraryRecovery.sessionUnconfirmed')), 'pending execution has no terminal outcome yet');
 assert.ok(!pendingMarkup.includes(h.i18n.t('libraryRecovery.completed')));
 const buttons = nodes(h.draw()).filter(n => n.type === 'button');
 assert.ok(!buttons.some(n => n.props.children === h.i18n.t('libraryRecovery.execute')));
 assert.ok(!buttons.some(n => n.props.children === h.i18n.t('libraryRecovery.reopen')));
 assert.equal(h.calls.filter(c => c[0] === 'execute').length, 1);
 h.executions[0].reject(new Error('fixture failure')); await h.settle();
 const failedMarkup = h.text();
 assert.ok(failedMarkup.includes(h.i18n.t('libraryRecovery.executionFailed')));
 assert.ok(failedMarkup.includes(h.i18n.t('libraryRecovery.sessionUnconfirmed')));
 assert.ok(!failedMarkup.includes(h.i18n.t('libraryRecovery.unavailable')), 'prior analysis is not a fresh refused offer');
 assert.ok(!failedMarkup.includes(h.i18n.t('libraryRecovery.completed')));
 assert.equal(h.calls.filter(c => c[0] === 'reopen').length, 0);
 h.cleanup();
});

test('finite recovery progress is presentation only and late progress after cancellation is ignored',async()=>{
 const h=await host();h.click('analyze');h.pending[0].resolve(executable);await h.settle();h.click('execute');
 for(const stage of ['protecting','restoring','validating','finalizing']){
   h.executions[0].progress(stage);await h.settle();assert.ok(h.text().includes(h.i18n.t('libraryRecovery.progress.'+stage)));
   assert.equal(h.calls.filter(c=>c[0]==='reopen').length,0);assert.ok(!h.text().includes(h.i18n.t('libraryRecovery.completed')));
 }
 h.click('cancelExecution');await h.settle();const afterCancel=h.text();h.executions[0].progress('restoring');await h.settle();assert.equal(h.text(),afterCancel);
 h.executions[0].resolve({completed:true,backup_retained:true,requires_reopen:true});await h.settle();assert.ok(!h.text().includes(h.i18n.t('libraryRecovery.completed')));h.cleanup();
});
test('recovery service drops unknown progress and preserves the invocation result',async()=>{
 const calls=[],seen=[];class Channel{onmessage;}
 const {libraryRecoveryService:service}=compile('../src/services/libraryRecovery.ts',{'@tauri-apps/api/core':{Channel,invoke:(...args)=>{calls.push(args);return Promise.resolve({completed:true,backup_retained:true,requires_reopen:true})}}});
 const result=service.execute(7,'safe-plan',stage=>seen.push(stage));const progress=calls[0][1].progress;
 for(const stage of ['checking','/private/path','completed','reopen',{stage:'restoring'},'restoring'])progress.onmessage(stage);
 assert.deepEqual(seen,['checking','restoring']);assert.deepEqual(await result,{completed:true,backup_retained:true,requires_reopen:true});
});


for (const language of ['pt-BR', 'en']) test(`recovery exposes a busy indicator only while an operation is active in ${language}`, async () => {
 const h = await host(language);
 const status = () => nodes(h.draw()).find(node => node.props.role === 'status' && node.props['aria-live'] === 'polite');
 assert.equal(status().props['aria-busy'], false);
 h.click('analyze');
 assert.equal(status().props['aria-busy'], true);
 assert.ok(nodes(status()).some(node => node.props['aria-hidden'] === 'true'));
 h.pending[0].resolve(executable); await h.settle();
 assert.equal(status().props['aria-busy'], false);
 h.click('execute');
 assert.equal(status().props['aria-busy'], true);
 assert.ok(!h.text().includes(h.i18n.t('libraryRecovery.explanation')), 'the readonly check description must not describe a modifying execution');
 h.executions[0].reject(new Error('fixture failure')); await h.settle();
 assert.equal(status().props['aria-busy'], false);
 assert.ok(h.text().includes(h.i18n.t('libraryRecovery.sessionUnconfirmed')));
 assert.equal(h.calls.filter(call => call[0] === 'reopen').length, 0);
 h.cleanup();
});
