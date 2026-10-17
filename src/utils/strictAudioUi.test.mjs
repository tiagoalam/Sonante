import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import {renderToStaticMarkup} from 'react-dom/server';
import i18next from 'i18next';
import {audioRestartExpected,localFoldersOnlyChanged} from './audioConfigChange.ts';

const app=readFileSync(new URL('../App.tsx',import.meta.url),'utf8');
const settings=readFileSync(new URL('../components/SettingsModal.tsx',import.meta.url),'utf8');
function find(source,match){const ast=ts.createSourceFile('source.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let result;function visit(n){if(match(n,ast))result=n;ts.forEachChild(n,visit)}visit(ast);assert.ok(result);return result;}
function compile(expression,values){const body=ts.transpileModule(`const result = (${expression});`,{fileName:'fixture.tsx',compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText;return new Function('exports','require',...Object.keys(values),body+'return result;')({},name=>{assert.equal(name,'react/jsx-runtime');return jsx},...Object.values(values));}
const handler=find(settings,n=>ts.isVariableDeclaration(n)&&n.name.getText()==='handleSave').initializer.getText();
const saved=find(app,n=>ts.isJsxAttribute(n)&&n.name.getText()==='onSaved').initializer.expression.getText();
const notice=find(app,n=>ts.isJsxExpression(n)&&n.expression?.getText().startsWith('(configSavePhase || configSaveNotice) &&')).expression.getText();

for(const lng of ['pt-BR','en'])test(`real Save handler publishes a partial warning only after completion; translated App notice (${lng})`,async()=>{
 const i18n=i18next.createInstance();await i18n.init({lng,resources:{[lng]:{translation:JSON.parse(readFileSync(new URL(`../locales/${lng}.json`,import.meta.url)))}},fallbackLng:false});const t=i18n.t.bind(i18n);
 for(const warning of [undefined,'position_not_restored']){
  const events=[];let visible=null;
  const config={first_run:false,audio_output_type:'alsa',alsa_device:'hw:Fixture,0',local_folders:[],dop_enabled:false,audio_buffer_size_kb:4096,replay_gain:'off'};
  const onSaved=compile(saved,{invalidateStatusRequests:()=>events.push('invalidate'),configService:{getConfig:async()=>config},setConfig:()=>events.push('confirmedConfig'),logError:()=>assert.fail('unexpected error'),refreshPlexFavorites:()=>events.push('favorites'),setConfigSaveNotice:n=>{visible=n;events.push('notice')}});
  const save=compile(handler,{config,saveInProgressRef:{current:false},originalConfigRef:{current:config},availability:{results:[]},availabilityQueryRef:{current:{invalidate(){}}},newPhysicalOutputChoice:()=>false,audioRestartExpected,localFoldersOnlyChanged,onStartLocalFoldersSave:()=>assert.fail('not a library save'),onSaveStarted:()=>true,setSaving:()=>{},configService:{saveConfig:async()=>{events.push('applied');return{status:'completed',...(warning?{warning}:{})}},getConfig:async()=>config},onSaveAccepted:()=>assert.fail('not accepted'),onSaved,onClose:()=>events.push('closed'),onSaveFinished:()=>events.push('finished'),formatConfigError:()=>'',logError:()=>assert.fail('unexpected error'),setConfig:()=>{},t,alert:()=>assert.fail('partial application is not an error alert')});
  await save();
  assert.equal(visible?.positionNotRestored===true,!!warning);
  if(warning){assert.ok(events.indexOf('notice')>events.indexOf('applied'));const element=compile(notice,{configSavePhase:null,configSaveNotice:visible,localSavePhase:null,t,setConfigSaveNotice:()=>{},RefreshCw:icons.RefreshCw,CheckCircle2:icons.CheckCircle2,CircleAlert:icons.CircleAlert,X:icons.X});const html=renderToStaticMarkup(element);assert.match(html,/role="alert"/);assert.ok(html.includes(t('settings.outputPositionNotRestored')));assert.doesNotMatch(html,/settings\.|SECRET|ACK/);}
  assert.equal(events.filter(x=>x==='applied').length,1);assert.ok(events.includes('closed')&&events.includes('finished'));
 }
});
