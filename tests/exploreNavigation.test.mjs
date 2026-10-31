import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import * as icons from "lucide-react";
import { configuredExploreSources, exploreSourceFor } from "../src/utils/exploreNavigation.ts";
import { createMainSectionPreference, MAIN_SECTION_STORAGE_KEY } from "../src/utils/appBootstrap.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const ast = ts.createSourceFile("App.tsx", app, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function initializer(name) {
  let result;
  const visit = node => {
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === name) result = node.initializer.getText(ast);
    ts.forEachChild(node, visit);
  };
  visit(ast); assert.ok(result, name); return result;
}
function compile(expression, env) {
  const js = ts.transpileModule(`const value = (${expression});`, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
  return new Function(...Object.keys(env), js + "return value;")(...Object.values(env));
}

test("Explore uses configured sources, including Plex before its catalog or network responds", () => {
  for (const [local_folders, plex_token, expected] of [
    [[], "", []], [["fixture"], "", ["local"]], [[], " token ", ["plex"]],
    [["fixture"], "token", ["local", "plex"]], [[], "  ", []],
  ]) {
    const sources = configuredExploreSources({ local_folders, plex_token });
    assert.deepEqual(sources, expected);
    for (const preferred of ["local", "plex"]) {
      assert.equal(exploreSourceFor(sources, preferred), expected.includes(preferred) ? preferred : expected[0] ?? "local");
    }
  }
});

test("source tabs appear only inside Explore with both configured sources", () => {
  for (const [centralSection, plexArea, exploring] of [
    ["local", "library", false], ["plex", "library", false], ["plex", "discover", true],
    ["explore", "library", true], ["favorites", "discover", false], ["playlists", "library", false],
  ]) {
    const isExploreSection = compile(initializer("isExploreSection"), { centralSection, plexArea });
    assert.equal(isExploreSection, exploring);
    for (const exploreSources of [[], ["local"], ["plex"], ["local", "plex"]]) {
      assert.equal(compile(initializer("showExploreTabs"), { isExploreSection, exploreSources }), exploring && exploreSources.length === 2);
    }
  }
});

test("actual Explore navigation routes each source and restart restores its principal library", () => {
  for (const sources of [[], ["local"], ["plex"], ["local", "plex"]]) {
    const ctx = { saved: null, section: null, resets: 0, area: "library", source: "local" };
    const preference = createMainSectionPreference(section => ctx.section = section, () => ({
      getItem: () => ctx.saved, setItem(key, value) { assert.equal(key, MAIN_SECTION_STORAGE_KEY); ctx.saved = value; },
    }));
    const config = { first_run: false, plex_token: sources.includes("plex") ? "fixture" : "" };
    preference.restore(config);
    for (const preferred of ["local", "plex", "local"]) {
      const navigation = () => compile(initializer("navigateExplore"), {
        exploreSources: sources, exploreSource: ctx.source, exploreSourceFor,
        resetAllNavigation() { ctx.resets++; }, setExploreSource(value) { ctx.source = value; },
        setPlexArea(value) { ctx.area = value; }, mainSectionPreference: preference,
      });
      navigation()(preferred);
      const actual = exploreSourceFor(sources, preferred);
      assert.equal(ctx.section, actual === "plex" ? "plex" : "explore");
      assert.equal(ctx.source, actual);
      assert.equal(ctx.area, actual === "plex" ? "discover" : "library");
      assert.equal(ctx.saved, actual);
      preference.restore(config);
      assert.equal(ctx.section, actual);
      navigation()();
      assert.equal(ctx.section, actual === "plex" ? "plex" : "explore");
    }
    assert.equal(ctx.resets, 6);
  }
});

test("old saved Local Explore migrates at bootstrap without writing or entering Explore", () => {
  let section; const writes = [];
  const preference = createMainSectionPreference(value => section = value, () => ({ getItem: () => "explore", setItem: (...args) => writes.push(args) }));
  for (const plex_token of ["", "fixture"]) {
    preference.restore({ first_run: false, plex_token });
    assert.equal(section, "local"); assert.deepEqual(writes, []);
  }
});

const component = readFileSync(new URL("../src/components/ExploreSourceTabs.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(component, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
const jsxRuntime = await import("react/jsx-runtime");
const sharedComponent = readFileSync(new URL("../src/components/LibrarySourceTabs.tsx", import.meta.url), "utf8");
const sharedCompiled = ts.transpileModule(sharedComponent, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
const sharedExports = {};
new Function("require", "exports", sharedCompiled)(name => ({ react: { __esModule: true, default: React }, "react/jsx-runtime": jsxRuntime, "lucide-react": icons })[name], sharedExports);

function tabs(language, props) {
  const dictionary = JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), "utf8"));
  const dependencies = { react: { __esModule: true, default: React }, "react/jsx-runtime": jsxRuntime, "lucide-react": icons, "./LibrarySourceTabs": sharedExports,
    "react-i18next": { useTranslation: () => ({ t: key => key.split(".").reduce((value, part) => value?.[part], dictionary) ?? key }) } };
  const exports = {};
  new Function("require", "exports", compiled)(name => { assert.ok(name in dependencies, name); return dependencies[name]; }, exports);
  const tree = exports.ExploreSourceTabs(props);
  return tree.type(tree.props);
}

test("visible source tabs have translated labels, selection, and keyboard/click navigation", () => {
  for (const language of ["en", "pt-BR"]) for (const source of ["local", "plex"]) {
    const calls = [], focused = [];
    const tree = tabs(language, { source, onChange: value => calls.push(value) });
    const html = renderToStaticMarkup(tree);
    assert.match(html, /role="tablist"/); assert.match(html, /role="tab"/);
    assert.match(html, />Local</); assert.match(html, />Plex</); assert.doesNotMatch(html, /explore\.source/);
    const buttons = React.Children.toArray(tree.props.children.props.children);
    for (const button of buttons) {
      const selected = button.props.id === `explore-source-${source}`;
      assert.equal(button.props["aria-selected"], selected);
      assert.equal(button.props.tabIndex, selected ? 0 : -1);
      button.props.onClick();
    }
    assert.deepEqual(calls, [source === "local" ? "plex" : "local"]);
    const selected = buttons.find(button => button.props["aria-selected"]);
    for (const key of ["ArrowRight", "ArrowLeft", "Home", "End"]) {
      let prevented = false, stopped = false;
      selected.props.onKeyDown({ key, preventDefault() { prevented = true; }, stopPropagation() { stopped = true; },
        currentTarget: { parentElement: { querySelector(selector) { return { focus() { focused.push(selector); } }; } } } });
      assert.equal(prevented, true); assert.equal(stopped, true);
    }
    assert.deepEqual(calls, Array(4).fill(source === "local" ? "plex" : "local"));
    assert.deepEqual(focused, [source === "local" ? "#explore-source-plex" : "#explore-source-local",
      source === "local" ? "#explore-source-plex" : "#explore-source-local", "#explore-source-local", "#explore-source-plex"]);
  }
});
