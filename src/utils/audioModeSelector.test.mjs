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
await i18n.init({lng:"en",resources:Object.fromEntries(["en","pt-BR"].map(language=>[language,{translation:JSON.parse(readFileSync(new URL(`../locales/${language}.json`,import.meta.url),"utf8"))}]))});
const windowsSource=readFileSync(new URL("../../windows/src/main.tsx",import.meta.url),"utf8");
const source=readFileSync(new URL("../components/AudioModeSelector.tsx",import.meta.url),"utf8");
const code=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const exports={};new Function("require","exports",code)(name=>name==="react/jsx-runtime"?jsx:name==="react-i18next"?{useTranslation:()=>({t:i18n.t.bind(i18n)})}:name==="lucide-react"?icons:assert.fail(name),exports);
const nodes=tree=>Array.isArray(tree)?tree.flatMap(nodes):!tree||typeof tree!=="object"?[]:[tree,...nodes(tree.props?.children)];

test("shared and exclusive are the only compact, responsive output cards",async()=>{
 for(const language of ["en","pt-BR"]){
  await i18n.changeLanguage(language);
  const tree=exports.AudioModeSelector({selected:"equalized",onSelect:assert.fail,pendingSave:true});
  const cards=nodes(tree).filter(node=>node.type==="button"&&node.props["data-audio-mode"]);
  assert.deepEqual(cards.map(card=>card.props["data-audio-mode"]),["shared","direct"]);
  assert.equal(cards[0].props["aria-pressed"],true,"EQ is the Shared route, not a third output card");
  assert.equal(cards[0].props["data-pending-save"],"true");
  assert.match(tree.props.className,/grid-cols-1[\s\S]*sm:grid-cols-2/);
  const html=renderToStaticMarkup(tree);
  assert.ok(html.includes(i18n.t("audioExperience.mode.shared")));
  assert.ok(html.includes(i18n.t("audioExperience.mode.direct")));
  assert.doesNotMatch(html,/Equalized \(EQ\)/);
 }
 await i18n.changeLanguage("en");
});

test("Windows wording stays platform-specific without promising exclusive WASAPI",()=>{
 assert.match(windowsSource,/alsaDevice:"Windows audio device"/);
 assert.match(windowsSource,/direct:"Direct access to the selected endpoint\. This preview does not confirm exclusivity\."/);
 assert.match(windowsSource,/direct:"Acesso direto ao endpoint selecionado\. Esta prévia não confirma exclusividade\."/);
 assert.doesNotMatch(windowsSource,/mode:\{shared:"WASAPI Shared · Windows",direct:"WASAPI Exclusive/);
});
