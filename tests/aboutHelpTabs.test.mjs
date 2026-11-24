import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import ts from "typescript";

const read = path => readFileSync(new URL("../" + path, import.meta.url), "utf8");
const nodes = (tree, predicate) => Array.isArray(tree)
  ? tree.flatMap(node => nodes(node, predicate))
  : !tree || typeof tree !== "object"
    ? []
    : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const compile = (path, dependencies) => {
  const output = ts.transpileModule(read(path), { compilerOptions: {
    module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  const exports = {};
  new Function("require", "exports", output)(name => {
    assert.ok(name in dependencies, name);
    return dependencies[name];
  }, exports);
  return exports;
};
const dictionary = language => {
  const data = JSON.parse(read("src/locales/" + language + ".json"));
  return (key, values = {}) => key.split(".").reduce((value, part) => value?.[part], data)
    ?.replace(/\{\{(\w+)\}\}/g, (_match, name) => String(values[name] ?? "")) ?? key;
};

function fixture(language = "en", initialTab = "about") {
  let cursor = 0;
  const slots = [];
  const effects = [];
  let closeCount = 0;
  const t = dictionary(language);
  const hooks = {
    ...React,
    useId: () => "about-help-test",
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
      return [slots[index], value => { slots[index] = typeof value === "function" ? value(slots[index]) : value; }];
    },
    useRef(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = { current: initial };
      return slots[index];
    },
    useEffect(effect) { effects.push(effect); },
  };
  const shortcut = compile("src/components/KeyboardShortcutsHelp.tsx", {
    "react/jsx-runtime": jsx,
    "react-i18next": { useTranslation: () => ({ t }) },
  }).KeyboardShortcutsHelp;
  const About = compile("src/components/AboutModal.tsx", {
    react: hooks,
    "react/jsx-runtime": jsx,
    "react-i18next": { useTranslation: () => ({ t }) },
    "lucide-react": new Proxy({}, { get: (_target, icon) => icon }),
    "@tauri-apps/plugin-opener": { openUrl: async () => {} },
    "./LocalDiagnostics": { LocalDiagnostics: () => null },
    "./KeyboardShortcutsHelp": { KeyboardShortcutsHelp: shortcut },
  }).AboutModal;
  return {
    effects,
    closeCount: () => closeCount,
    render() {
      cursor = 0;
      effects.length = 0;
      return About({ onClose: () => { closeCount++; }, initialTab });
    },
  };
}

test("About / Help opens on About and keeps the shortcuts and instructions in an accessible tab", () => {
  for (const language of ["en", "pt-BR"]) {
    const f = fixture(language);
    const tree = f.render();
    const dialog = nodes(tree, node => node.props?.role === "dialog")[0];
    const tabs = nodes(tree, node => node.props?.role === "tab");
    const panels = nodes(tree, node => node.props?.role === "tabpanel");
    const messages = JSON.parse(read("src/locales/" + language + ".json"));

    assert.equal(dialog.props["aria-modal"], "true");
    assert.equal(dialog.props["aria-label"], messages.sidebar.about);
    assert.equal(tabs.length, 2);
    assert.equal(tabs[0].props.children, messages.about.tabAbout);
    assert.equal(tabs[1].props.children, messages.about.tabShortcuts);
    assert.equal(tabs[0].props["aria-selected"], true);
    assert.equal(tabs[1].props["aria-selected"], false);
    assert.deepEqual(tabs.map(tab => tab.props.tabIndex), [0, -1]);
    assert.equal(panels.find(panel => panel.props.id === tabs[0].props["aria-controls"]).props.hidden, false);
    assert.equal(panels.find(panel => panel.props.id === tabs[1].props["aria-controls"]).props.hidden, true);

    const shortcutsElement = nodes(panels[1], node => typeof node.type === "function")[0];
    const shortcuts = shortcutsElement.type();
    assert.equal(nodes(shortcuts, node => node.type === "kbd").length, 10);
    assert.ok(nodes(shortcuts, node => node.type === "p").some(node => node.props.children === messages.shortcuts.contextHint));
    const close = nodes(tree, node => node.type === "button" && node.props["aria-label"] === messages.about.close)[0];
    close.props.onClick();
    assert.equal(f.closeCount(), 1);
    assert.match(panels.find(panel => panel.props.hidden === false).props.className, /overflow-y-auto/);
    const shortcutsTree = fixture(language, "shortcuts").render();
    assert.match(nodes(shortcutsTree, node => node.props?.role === "tabpanel")
      .find(panel => panel.props.hidden === false).props.className, /overflow-y-auto/);
    assert.match(dialog.props.className, /max-h-\[90vh\]/);
  }
});

test("tabs support arrow and Home/End navigation and allow a direct shortcuts initial tab", () => {
  const f = fixture();
  let tree = f.render();
  const tabs = nodes(tree, node => node.props?.role === "tab");
  const focused = [];
  tabs.forEach(tab => { tab.props.ref.current = { focus: () => focused.push(tab.props.id) }; });
  const tablist = nodes(tree, node => node.props?.role === "tablist")[0];
  let prevented = 0;
  tablist.props.onKeyDown({ key: "ArrowRight", preventDefault: () => prevented++, stopPropagation() {} });
  assert.deepEqual(focused, [tabs[1].props.id]);
  tree = f.render();
  assert.deepEqual(nodes(tree, node => node.props?.role === "tab").map(tab => tab.props["aria-selected"]), [false, true]);

  const clickFixture = fixture();
  tree = clickFixture.render();
  nodes(tree, node => node.props?.role === "tab")[1].props.onClick();
  tree = clickFixture.render();
  assert.deepEqual(nodes(tree, node => node.props?.role === "tab").map(tab => tab.props["aria-selected"]), [false, true]);

  const direct = fixture("en", "shortcuts").render();
  assert.deepEqual(nodes(direct, node => node.props?.role === "tab").map(tab => tab.props["aria-selected"]), [false, true]);
  const reopened = fixture().render();
  assert.deepEqual(nodes(reopened, node => node.props?.role === "tab").map(tab => tab.props["aria-selected"]), [true, false]);
  assert.equal(prevented, 1);
});

test("dialog traps focus, handles Escape, restores the opener focus, and isolates global shortcuts", () => {
  const priorDocument = globalThis.document;
  const priorHTMLElement = globalThis.HTMLElement;
  class FakeHTMLElement {
    isConnected = true;
    focused = 0;
    focus() { this.focused++; }
    closest() { return null; }
  }
  try {
    globalThis.HTMLElement = FakeHTMLElement;
    const priorFocus = new FakeHTMLElement();
    globalThis.document = { activeElement: priorFocus };
    const f = fixture();
    const tree = f.render();
    const dialog = nodes(tree, node => node.props?.role === "dialog")[0];
    const tabs = nodes(tree, node => node.props?.role === "tab");
    const initialTab = new FakeHTMLElement();
    tabs[0].props.ref.current = initialTab;
    const cleanup = f.effects[0]();
    assert.equal(initialTab.focused, 1);

    const first = new FakeHTMLElement();
    const last = new FakeHTMLElement();
    const dialogNode = {
      querySelectorAll: () => [first, last],
      contains: node => node === first || node === last,
      focus() {},
    };
    let prevented = 0;
    let stopped = 0;
    globalThis.document.activeElement = last;
    dialog.props.onKeyDown({ key: "Tab", shiftKey: false, currentTarget: dialogNode,
      preventDefault: () => prevented++, stopPropagation: () => stopped++ });
    assert.equal(first.focused, 1);
    globalThis.document.activeElement = first;
    dialog.props.onKeyDown({ key: "Tab", shiftKey: true, currentTarget: dialogNode,
      preventDefault: () => prevented++, stopPropagation: () => stopped++ });
    assert.equal(last.focused, 1);
    dialog.props.onKeyDown({ key: "Escape", currentTarget: dialogNode,
      preventDefault: () => prevented++, stopPropagation: () => stopped++ });
    assert.equal(f.closeCount(), 1);
    assert.equal(stopped, 3);
    cleanup();
    assert.equal(priorFocus.focused, 1);
    assert.equal(prevented, 3);
  } finally {
    if (priorDocument === undefined) delete globalThis.document;
    else globalThis.document = priorDocument;
    if (priorHTMLElement === undefined) delete globalThis.HTMLElement;
    else globalThis.HTMLElement = priorHTMLElement;
  }
});

test("the sidebar has one About / Help entry and no independent shortcuts item", () => {
  const app = read("src/App.tsx");
  const en = JSON.parse(read("src/locales/en.json"));
  const pt = JSON.parse(read("src/locales/pt-BR.json"));
  assert.doesNotMatch(app, /<KeyboardShortcutsHelp\s*\/>/);
  assert.match(app, /label=\{t\("sidebar\.about"\)\}/);
  assert.match(app, /<AboutModal onClose=/);
  assert.equal(en.sidebar.about, "About / Help");
  assert.equal(pt.sidebar.about, "Sobre / Ajuda");
});
