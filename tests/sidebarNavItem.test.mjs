import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import { renderToStaticMarkup } from "react-dom/server";

const source = readFileSync(new URL("../src/components/SidebarNavItem.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  fileName: "SidebarNavItem.tsx",
  compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;
const exports = {};
new Function("require", "exports", compiled)(name => {
  if (name === "react") return { ...React, default: React };
  if (name === "react/jsx-runtime") return jsx;
  if (name === "../utils/frontendDiagnostics") return { traceInteractionAction: (_action, callback) => callback() };
  if (name === "lucide-react") return {};
  throw new Error(`Unexpected import: ${name}`);
}, exports);

const icon = props => React.createElement("svg", props);
const renderItem = props => renderToStaticMarkup(React.createElement(exports.SidebarNavItem, {
  icon,
  label: "Available item",
  onClick() {},
  ...props,
}));

test("available sidebar items keep primary contrast, visible focus, and selected state", () => {
  for (const sourceEnabled of [undefined, true]) {
    const html = renderItem({ sourceEnabled });
    assert.match(html, /hover:bg-\[var\(--theme-surface-raised,#202020\)\]/);
    assert.match(html, /text-\[color:var\(--theme-fg-primary,#ffffff\)\]/);
    assert.match(html, /focus-visible:ring-1 focus-visible:ring-\[color:var\(--theme-border-accent,#E5A00D\)\]/);
    assert.doesNotMatch(html, /<span class="[^"]*text-\[color:var\(--theme-fg-muted/);
  }

  const active = renderItem({ sourceEnabled: true, active: true });
  assert.match(active, /aria-current="page"/);
  assert.match(active, /bg-\[var\(--theme-surface-raised,#242424\)\] font-semibold/);
  assert.match(active, /text-\[color:var\(--theme-fg-accent,#E5A00D\)\]/);
  assert.match(active, /aria-hidden="true" class="absolute inset-y-2 left-0 w-0\.5 rounded-r bg-\[var\(--theme-fg-accent,#E5A00D\)\]"/);
});

test("a disabled source stays muted but clickable so its settings remain accessible", () => {
  const html = renderItem({ sourceEnabled: false });
  assert.match(html, /text-\[color:var\(--theme-fg-muted,#8b8b8b\)\]/);
  assert.match(html, /<svg[^>]*class="[^"]*text-\[color:var\(--theme-fg-muted,#8b8b8b\)\]/);
  assert.match(html, /<span class="min-w-0 flex-1 truncate">Available item<\/span>/);
  assert.doesNotMatch(html, /<button[^>]*\sdisabled(?:="")?(?:\s|>)/);
  assert.doesNotMatch(html, /aria-disabled="true"/);
});

test("a selected disabled source keeps its selected marker while its label stays muted", () => {
  const html = renderItem({ sourceEnabled: false, active: true });
  assert.match(html, /aria-current="page"/);
  assert.match(html, /bg-\[var\(--theme-surface-raised,#242424\)\] font-semibold/);
  assert.match(html, /bg-\[var\(--theme-fg-accent,#E5A00D\)\]/);
  assert.match(html, /<svg[^>]*class="[^"]*text-\[color:var\(--theme-fg-muted,#8b8b8b\)\]/);
  assert.doesNotMatch(html, /<button[^>]*\sdisabled(?:="")?(?:\s|>)/);
});

test("only an actually disabled action is attenuated and non-interactive", () => {
  const html = renderItem({ sourceEnabled: true, disabled: true });
  assert.match(html, /<button[^>]*disabled=""/);
  assert.match(html, /aria-disabled="true"/);
  assert.match(html, /cursor-not-allowed opacity-55 disabled:hover:bg-transparent/);
  assert.match(html, /focus-visible:ring-1 focus-visible:ring-/);
});
