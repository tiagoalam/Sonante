import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import i18next from "i18next";
import { renderToStaticMarkup } from "react-dom/server";

const i18n=i18next.createInstance();
await i18n.init({lng:"en",resources:Object.fromEntries(["en","pt-BR"].map(language=>[language,{translation:JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`,import.meta.url),"utf8"))}]))});
const source=readFileSync(new URL("../src/components/OpraCatalogBrowser.tsx",import.meta.url),"utf8");
const code=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const errorSource=readFileSync(new URL("../src/utils/opraErrors.ts",import.meta.url),"utf8");const errorCode=ts.transpileModule(errorSource,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const opraErrors={};new Function("exports",errorCode)(opraErrors);
const searchSource=readFileSync(new URL("../src/utils/opraSearch.ts",import.meta.url),"utf8");const searchCode=ts.transpileModule(searchSource,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const opraSearch={};new Function("require","exports",searchCode)(()=>({}),opraSearch);
const calls=[];
const exports={};
new Function("require","exports",code)(name=>name==="react"?React:name==="react/jsx-runtime"?jsx:name==="react-i18next"?{useTranslation:()=>({t:i18n.t.bind(i18n)})}:name==="lucide-react"?icons:name==="../services/equalizer"?{equalizerService:{searchOpraProfiles:(...args)=>{calls.push(args);return Promise.resolve({})}}}:name==="../utils/opraErrors"?opraErrors:name==="../utils/opraSearch"?opraSearch:assert.fail(name),exports);

test("OPRA browser is explicitly attributed and does not query or apply profiles on render",async()=>{
 for(const language of ["en","pt-BR"]){
  await i18n.changeLanguage(language);
  const html=renderToStaticMarkup(React.createElement(exports.OpraCatalogBrowser,{}));
  assert.ok(html.includes(i18n.t("equalizer.library.opraTitle")));
  assert.ok(html.includes(i18n.t("equalizer.library.opraAttribution")));
  assert.ok(html.includes(i18n.t("equalizer.library.opraPrivacy")));
  assert.match(html, /aria-label="Find headphone profiles online \(OPRA\)|aria-label="Buscar perfis de fones online \(OPRA\)/);
  assert.match(html,/href="https:\/\/github\.com\/opra-project\/OPRA"/);
  assert.match(html,/rel="noreferrer"/);
  assert.doesNotMatch(html,/Apply OPRA|Aplicar perfil OPRA|Save OPRA profile|Salvar perfil OPRA/);
 }
 assert.match(source,/result\.dataset_license/);
 assert.match(source,/profile\.profile_id/);
 assert.match(source,/opraConversionBlocked/);
 assert.doesNotMatch(source,/onClick=.*(?:apply_equalizer|edit_equalizer|saveOpra)/);
 assert.deepEqual(calls,[],"opening AutoEQ must not initiate network access");
 await i18n.changeLanguage("en");
});
test("failed OPRA refresh preserves the results already displayed", async () => {
  let calls = 0;
  let states = [], refs = [], effects = [], si = 0, ri = 0;
  let mounted = false;
  const cleanups = [];
  const hooks = {
    ...React,
    useState(value) { const i=si++; if (!(i in states)) states[i]=typeof value==="function"?value():value; return [states[i], next=>states[i]=typeof next==="function"?next(states[i]):next]; },
    useRef(value) { return refs[ri++] ??= {current:value}; },
    useEffect(effect) { effects.push(effect); },
  };
  const utilitySource=readFileSync(new URL("../src/utils/opraErrors.ts",import.meta.url),"utf8");
  const utilityCode=ts.transpileModule(utilitySource,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
  const utility={};new Function("exports",utilityCode)(utility);
  const component={};
  new Function("require","exports",code)(name=>name==="react"?hooks:name==="react/jsx-runtime"?jsx:name==="react-i18next"?{useTranslation:()=>({t:i18n.t.bind(i18n)})}:name==="lucide-react"?icons:name==="../services/equalizer"?{equalizerService:{searchOpraProfiles:async()=>{calls++;if(calls===2)throw "OPRA_NETWORK_UNAVAILABLE";return{snapshot_sha256:"a".repeat(64),dataset_license:"CC-BY-SA-4.0",products:[],truncated:false}}}}:name==="../utils/opraErrors"?utility:name==="../utils/opraSearch"?opraSearch:assert.fail(name),component);
  const render=()=>{si=ri=0;effects=[];const tree=component.OpraCatalogBrowser({});if(!mounted){mounted=true;for(const effect of effects){const cleanup=effect();if(typeof cleanup==="function")cleanups.push(cleanup)}}return tree;};
  const nodes=(tree,predicate)=>Array.isArray(tree)?tree.flatMap(node=>nodes(node,predicate)):!tree||typeof tree!=="object"?[]:tree.type?.name==="ProductResults"?nodes(component.ProductResults(tree.props),predicate):[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)];
  const flush=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
  let tree=render();nodes(tree,node=>node.type==="input")[0].props.onChange({target:{value:"synthetic model"}});tree=render();
  nodes(tree,node=>node.type==="form")[0].props.onSubmit({preventDefault(){}});await flush();tree=render();
  assert.ok(nodes(tree,node=>node.type==="p").some(node=>node.props.children===i18n.t("equalizer.library.opraNoResults")));
  nodes(tree,node=>node.type==="button"&&node.props.type==="button")[0].props.onClick();await flush();tree=render();
  assert.ok(nodes(tree,node=>node.type==="p").some(node=>node.props.children===i18n.t("equalizer.library.opraNoResults")));
  assert.equal(nodes(tree,node=>node.props?.["data-opra-error"]==="networkUnavailable").length,1);
  assert.equal(calls,2);
  cleanups.forEach(cleanup=>cleanup());
});

test("AutoEQ ranks are rendered as selectable headphone cards with details in a separate panel",async()=>{
  const profiles=(prefix,author)=>[{profile_id:`${prefix}_profile`,author,details:"Synthetic measurement",source_link:"https://example.invalid/source",overall_gain_db:0,bands:[{filter_type:"peak_dip",frequency:1000,gain_db:1,q:1,slope:null}],compatibility:"unverified",compatibility_reasons:[]}];
  const products=[
    {product_id:"hifiman_edition_xs",vendor_id:"hifiman",product_name:"Edition XS",vendor_name:"HIFIMAN",product_type:"headphones",subtype:"over_the_ear",profile_count:17,profiles:profiles("edition","Community Author")},
    {product_id:"hifiman_he6",vendor_id:"hifiman",product_name:"HE-6",vendor_name:"HIFIMAN",product_type:"headphones",subtype:"over_the_ear",profile_count:4,profiles:profiles("he6","Another Author")},
  ];
  let states=[],refs=[],effects=[],si=0,ri=0,mounted=false,capturedQuery="";const cleanups=[],added=[];
  const hooks={...React,useState(value){const i=si++;if(!(i in states))states[i]=typeof value==="function"?value():value;return[states[i],next=>states[i]=typeof next==="function"?next(states[i]):next]},useRef(value){return refs[ri++]??={current:value}},useEffect(effect){effects.push(effect)}};
  const component={};
  new Function("require","exports",code)(name=>name==="react"?hooks:name==="react/jsx-runtime"?jsx:name==="react-i18next"?{useTranslation:()=>({t:i18n.t.bind(i18n)})}:name==="lucide-react"?icons:name==="../services/equalizer"?{equalizerService:{searchOpraProfiles:async query=>{capturedQuery=query;return{snapshot_sha256:"b".repeat(64),dataset_license:"CC-BY-SA-4.0",products,truncated:false}}}}:name==="../utils/opraErrors"?opraErrors:name==="../utils/opraSearch"?opraSearch:assert.fail(name),component);
  const render=()=>{si=ri=0;effects=[];const tree=component.OpraCatalogBrowser({onAddProduct:async product=>added.push(product)});if(!mounted){mounted=true;for(const effect of effects){const cleanup=effect();if(typeof cleanup==="function")cleanups.push(cleanup)}}return tree};
  const nodes=(tree,predicate)=>Array.isArray(tree)?tree.flatMap(node=>nodes(node,predicate)):!tree||typeof tree!=="object"?[]:tree.type?.name==="ProductResults"?nodes(component.ProductResults(tree.props),predicate):[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)];
  const flush=async()=>{for(let i=0;i<16;i++)await Promise.resolve()};
  let tree=render();nodes(tree,node=>node.type==="input")[0].props.onChange({target:{value:"HIFIMAN Edition XS"}});tree=render();nodes(tree,node=>node.type==="form")[0].props.onSubmit({preventDefault(){}});await flush();tree=render();
  assert.equal(capturedQuery,"HIFIMAN Edition XS");
  assert.deepEqual(nodes(tree,node=>node.props?.["data-opra-product"]).map(node=>node.props["data-opra-product"]),["hifiman_edition_xs","hifiman_he6"]);
  assert.equal(nodes(tree,node=>node.props?.["data-opra-selected-product"]).length,0,"technical detail is hidden before product selection");
  nodes(tree,node=>node.type==="button"&&node.props["aria-label"]===i18n.t("equalizer.library.opraSelectProduct",{product:"Edition XS"}))[0].props.onClick();tree=render();
  assert.equal(nodes(tree,node=>node.props?.["data-opra-selected-product"])[0].props["data-opra-selected-product"],"hifiman_edition_xs");
  assert.equal(nodes(tree,node=>node.type==="h3"&&node.props.children==="Edition XS").length,1,"the selected product has a dedicated detail heading");
  const other=nodes(tree,node=>node.type==="button"&&node.props["aria-label"]===i18n.t("equalizer.library.opraSelectProduct",{product:"HE-6"}))[0];other.props.onClick();tree=render();
  assert.equal(nodes(tree,node=>node.props?.["data-opra-selected-product"])[0].props["data-opra-selected-product"],"hifiman_he6");
  const addButton=nodes(tree,node=>node.type==="button"&&node.props.children===i18n.t("autoeq.addToMyEquipment")).at(-1);addButton.props.onClick();await flush();assert.equal(added[0].product_id,"hifiman_he6");
  assert.match(readFileSync(new URL("../src/components/OpraCatalogBrowser.tsx",import.meta.url),"utf8"),/onSelect=\{setSelectedProductId\}/);
  assert.doesNotMatch(readFileSync(new URL("../src/components/OpraCatalogBrowser.tsx",import.meta.url),"utf8"),/apply_equalizer|edit_equalizer|saveOpra/);
  cleanups.forEach(cleanup=>cleanup());
});
