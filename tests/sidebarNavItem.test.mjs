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

test("available sidebar items use secondary text and retain primary hover and active styling", () => {
  for (const sourceEnabled of [undefined, true]) {
    const html = renderItem({ sourceEnabled });
    assert.match(html, /text-\[color:var\(--theme-fg-secondary,#CCCCCC\)\] hover:bg-/);
    assert.match(html, /group-hover:text-\[color:var\(--theme-fg-primary,#ffffff\)\]/);
    assert.doesNotMatch(html, /<span class="[^"]*text-\[color:var\(--theme-fg-muted/);
  }

  const active = renderItem({ sourceEnabled: true, active: true });
  assert.match(active, /aria-current="page"/);
  assert.match(active, /bg-\[var\(--theme-surface-raised,#242424\)\] text-\[color:var\(--theme-fg-primary,#ffffff\)\]/);
  assert.match(active, /text-\[color:var\(--theme-fg-accent,#E5A00D\)\]/);
});

test("disabled sources keep muted text and icon colors", () => {
  const html = renderItem({ sourceEnabled: false });
  assert.match(html, /<svg[^>]*class="[^"]*text-\[color:var\(--theme-fg-muted,#777777\)\]/);
  assert.match(html, /<span class="[^"]*text-\[color:var\(--theme-fg-muted,#8A8A8A\)\]">Available item<\/span>/);
});
