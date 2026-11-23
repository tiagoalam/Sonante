import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import {renderToStaticMarkup} from 'react-dom/server';

const i18n=i18next.createInstance();
await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(language=>[language,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`,import.meta.url),'utf8'))}]))});
const source=readFileSync(new URL('../src/components/AutoEqView.tsx',import.meta.url),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};
new Function('require','exports',code)(name=>name==='react'?React:name==='react/jsx-runtime'?jsx:name==='react-i18next'?{useTranslation:()=>({t:i18n.t.bind(i18n)})}:name==='lucide-react'?icons:name==='../services/equalizer'?{equalizerService:{get:async()=>null,edit:async()=>null}}:name==='./OpraCatalogBrowser'?{OpraCatalogBrowser:()=>React.createElement('div',{'data-opra-browser':true})}:assert.fail(name),exports);

const preset=(id,name)=>({reference:{kind:'user',id},name});
const noop=async()=>true;

test('AutoEQ My Equipment provides one useful empty-state add flow in English and Portuguese',async()=>{
 for(const language of ['en','pt-BR']){
  await i18n.changeLanguage(language);
  const html=renderToStaticMarkup(React.createElement(exports.MyEquipment,{equipment:[],presets:[],busy:false,addOpen:true,category:'headphones',onToggleAdd:noop,onCategory:noop,onManual:noop,onEdit:noop,onOpenPreset:noop,onAddOpra:noop,t:i18n.t.bind(i18n)}));
  assert.ok(html.includes(i18n.t('autoeq.myEquipment')));
  assert.ok(html.includes(i18n.t('autoeq.addEquipment')));
  assert.ok(html.includes(i18n.t('autoeq.equipmentCategory')));
  assert.ok(html.includes(i18n.t('autoeq.category.speakers')));
  assert.ok(html.includes(i18n.t('autoeq.addManually')));
  assert.ok(html.includes('data-opra-browser'));
 }
 await i18n.changeLanguage('en');
});

test('category-specific add flow offers OPRA only for headphones',()=>{
 const base={onCategory:noop,onManual:noop,onAddOpra:noop,disabled:false,registeredProductIds:[],t:i18n.t.bind(i18n)};
 const headphones=renderToStaticMarkup(React.createElement(exports.AddEquipmentPanel,{...base,category:'headphones'}));
 const speakers=renderToStaticMarkup(React.createElement(exports.AddEquipmentPanel,{...base,category:'speakers'}));
 const other=renderToStaticMarkup(React.createElement(exports.AddEquipmentPanel,{...base,category:'other'}));
 assert.match(headphones,/data-opra-browser/);assert.doesNotMatch(speakers,/data-opra-browser/);assert.doesNotMatch(other,/data-opra-browser/);
});

test('saved equipment keeps multiple preset references and exposes the preferred one without applying audio',()=>{
 const equipment=[{id:'device-1',name:'Edition XS',category:'headphones',manufacturer:'HIFIMAN',opra_vendor_id:'hifiman',opra_product_id:'hifiman_edition_xs',presets:[{kind:'user',id:'warm'},{kind:'user',id:'neutral'}],preferred_preset:{kind:'user',id:'neutral'}}];
 const html=renderToStaticMarkup(React.createElement(exports.MyEquipment,{equipment,presets:[preset('warm','Warm'),preset('neutral','Neutral')],busy:false,addOpen:false,category:'headphones',onToggleAdd:noop,onCategory:noop,onManual:noop,onEdit:noop,onOpenPreset:noop,onAddOpra:noop,t:i18n.t.bind(i18n)}));
 assert.match(html,/Edition XS/);assert.match(html,/HIFIMAN/);assert.match(html,/Warm/);assert.match(html,/Neutral/);assert.match(html,/Open preferred preset in EQ Manual/);
 assert.doesNotMatch(html,/Apply OPRA|Aplicar perfil OPRA/);
 assert.doesNotMatch(source,/equalizerService\.apply\(/,'AutoEQ must not invoke DSP');
});
