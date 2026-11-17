import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

function compile(file, deps = {}) {
  const exports = {};
  const output = ts.transpileModule(readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"), {
    fileName: file,
    compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  new Function("require", "exports", output)(name => { assert.ok(name in deps, name); return deps[name]; }, exports);
  return exports;
}

const prompt = compile("utils/directVolumeChoicePrompt.ts");
let dictionary = JSON.parse(readFileSync(new URL("../src/locales/pt-BR.json", import.meta.url), "utf8"));
const t = key => key.split(".").reduce((value, part) => value?.[part], dictionary) ?? key;
const { DirectVolumeChoiceDialog } = compile("components/DirectVolumeChoiceDialog.tsx", {
  react: React,
  "react/jsx-runtime": jsx,
  "react-i18next": { useTranslation: () => ({ t }) },
  "../utils/directVolumeChoicePrompt": prompt,
});

test("Direct confirmation keeps concise bilingual device, volume and resume guidance", async () => {
  for (const language of ["pt-BR", "en"]) {
    dictionary = JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), "utf8"));
    const answer = prompt.requestDirectVolumeChoice();
    const html = renderToStaticMarkup(React.createElement(DirectVolumeChoiceDialog));
    assert.ok(html.includes(t("player.directVolumeChoiceTitle")));
    assert.ok(html.includes(t("player.directVolumeChoiceDescription")));
    assert.ok(html.includes(t("player.directVolumeChoiceSafety")));
    assert.ok(html.includes(t("player.directVolumeChoiceCancel")));
    assert.ok(html.includes(t("player.directVolumeChoiceContinue")));
    assert.ok(!html.includes(t("audioExperience.directAvailabilityNotice")), "the duplicate availability paragraph is omitted");
    prompt.respondToDirectVolumeChoice(true);
    assert.equal(await answer, true);
  }
});

test("confirmation requests remain serialized and cancellation resolves before the next prompt", async () => {
  const first = prompt.requestDirectVolumeChoice(), second = prompt.requestDirectVolumeChoice();
  assert.equal(prompt.getDirectVolumeChoicePromptSnapshot().open, true);
  prompt.respondToDirectVolumeChoice(false);
  assert.equal(await first, false);
  await Promise.resolve();
  assert.equal(prompt.getDirectVolumeChoicePromptSnapshot().open, true);
  prompt.respondToDirectVolumeChoice(true);
  assert.equal(await second, true);
  assert.equal(prompt.getDirectVolumeChoicePromptSnapshot().open, false);
});

test("App mounts the dialog and Direct authorization no longer calls the native confirmation window", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const config = readFileSync(new URL("../src/services/config.ts", import.meta.url), "utf8");
  assert.match(app, /<DirectVolumeChoiceDialog\s*\/>/);
  assert.match(config, /requestDirectVolumeChoice\(\)/);
  assert.doesNotMatch(config, /window\.confirm/);
});
