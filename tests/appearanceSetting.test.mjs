import test from 'node:test';import assert from 'node:assert/strict';import {readFileSync} from 'node:fs';import ts from 'typescript';import React from 'react';import * as jsx from 'react/jsx-runtime';import {renderToStaticMarkup} from 'react-dom/server';import i18next from 'i18next';import * as model from '../src/utils/appearance.ts';const {createAppearance,APPEARANCES}=model;
const i18n=i18next.createInstance();await i18n.init({lng:'en',resources:Object.fromEntries(['en','pt-BR'].map(l=>[l,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${l}.json`,import.meta.url),'utf8'))}]))});
const text=readFileSync(new URL('../src/components/AppearanceSetting.tsx',import.meta.url),'utf8'),code=ts.transpileModule(text,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
test('real bilingual appearance control changes only preference; no Save, audio command or remount',async()=>{
 const colors=[],store=createAppearance({origin:'ui',storage:()=>null,apply:v=>colors.push(v),onStorage:()=>()=>{}});store.start();const exports={};const deps={'react':{...React,useId:()=> 'appearance-fixture',useSyncExternalStore:(_s,get)=>get()},'react/jsx-runtime':jsx,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../services/appearance':{appearance:store},'../utils/appearance':model};new Function('require','exports',code)(n=>{assert.ok(n in deps,n);return deps[n]},exports);
 const nodes = tree => {const result=[];function walk(n){if(!React.isValidElement(n))return;result.push(n);React.Children.forEach(n.props.children,walk)}walk(tree);return result;};
 for(const lang of ['en','pt-BR']){
  await i18n.changeLanguage(lang);
  for(const mode of APPEARANCES){
   let tree=exports.AppearanceSetting(),html=renderToStaticMarkup(tree);
   assert.equal(tree.type,'fieldset');assert.match(html,/<legend[^>]*>/);
   assert.ok(html.includes(i18n.t('settings.appearanceDark')));assert.ok(html.includes(i18n.t('settings.appearanceLight')));
   assert.doesNotMatch(html,/settings\.|type="submit"|<select|<img/);assert.equal((html.match(/<svg/g)||[]).length,6);
   const radios=nodes(tree).filter(n=>n.type==='input');assert.equal(radios.length,6);assert.ok(radios.every(n=>n.props.type==='radio'&&n.props.name==='appearance-fixture'));
   radios.find(n=>n.props.value===mode).props.onChange();assert.equal(store.getSnapshot(),mode);
   tree=exports.AppearanceSetting();assert.equal(nodes(tree).filter(n=>n.type==='input'&&n.props.checked)[0].props.value,mode);
  }
 }
 assert.deepEqual(colors,['dark',...APPEARANCES,...APPEARANCES]);
});
