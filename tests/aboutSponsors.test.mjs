import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";

const source = readFileSync(new URL("../src/components/AboutModal.tsx", import.meta.url), "utf8");
const componentCode = ts.transpileModule(source, { compilerOptions: {
  module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020,
} }).outputText;
let slots = [], cursor = 0, opened;
const hooks = {
  ...React,
  useState(initial) {
    const index = cursor++;
    if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
    return [slots[index], value => { slots[index] = typeof value === "function" ? value(slots[index]) : value; }];
  },
};
const exports = {};
new Function("require", "exports", componentCode)(name => {
  if (name === "react") return { ...hooks, default: hooks, __esModule: true };
  if (name === "react/jsx-runtime") return jsx;
  if (name === "./LocalDiagnostics") return { LocalDiagnostics: () => null };
  if (name === "@tauri-apps/plugin-opener") return { openUrl: async url => { opened(url); } };
  if (name === "lucide-react") return new Proxy({}, { get: (_target, icon) => icon });
  if (name === "react-i18next") return { useTranslation: () => ({ t: key => key }) };
  throw new Error(`Unexpected AboutModal dependency: ${name}`);
}, exports);

function render() { cursor = 0; return exports.AboutModal({ onClose() {} }); }
function nodes(tree) {
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  if (!tree || typeof tree !== "object") return [];
  return [tree, ...nodes(tree.props?.children)];
}

test("About exposes a keyboard-accessible, discreet sponsor action through Tauri's external opener", async () => {
  opened = url => { opened.urls.push(url); };
  opened.urls = [];
  slots = [];
  const tree = render();
  const button = nodes(tree).find(node => node.type === "button" && node.props["aria-label"] === "about.supportDevelopment");
  assert.ok(button);
  assert.equal(button.props.type, "button");
  assert.match(button.props.className, /focus-visible:outline/);
  assert.match(button.props.className, /text-\[11px\]/);
  await button.props.onClick();
  assert.deepEqual(opened.urls, ["https://github.com/sponsors/tiagoalam"]);
  assert.match(readFileSync(new URL("../src-tauri/capabilities/default.json", import.meta.url), "utf8"), /"opener:default"/);
});

test("About reports external opener failure without affecting the dialog", async () => {
  opened = () => { throw new Error("browser unavailable"); };
  slots = [];
  let tree = render();
  const button = nodes(tree).find(node => node.type === "button" && node.props["aria-label"] === "about.supportDevelopment");
  await button.props.onClick();
  tree = render();
  assert.ok(nodes(tree).some(node => node.props?.role === "alert" && node.props.children === "about.sponsorOpenError"));
  assert.ok(nodes(tree).some(node => node.type === "h2" && node.props.children === "SONANTE"));
});

test("About displays the public candidate version without an internal-build label", () => {
  slots = [];
  const tree = render();
  assert.ok(nodes(tree).some(node => node.type === "span" && node.props.children === "about.version"));
  assert.doesNotMatch(source, /internalVersion|0\.6\.5/);
});

test("sponsor labels and opener failure are translated in English and Brazilian Portuguese", () => {
  for (const [locale, label, error] of [["en", "Support development", "Could not open GitHub Sponsors. Please try again."],
    ["pt-BR", "Apoiar o desenvolvimento", "Não foi possível abrir o GitHub Sponsors. Tente novamente."]]) {
    const messages = JSON.parse(readFileSync(new URL(`../src/locales/${locale}.json`, import.meta.url), "utf8"));
    assert.equal(messages.about.supportDevelopment, label);
    assert.equal(messages.about.sponsorOpenError, error);
    assert.equal(messages.about.version, "v{{version}}");
    assert.equal(messages.about.internalVersion, undefined);
  }
});
