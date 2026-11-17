import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";

const source = readFileSync(new URL("../components/SqliteSourcesSettings.tsx", import.meta.url), "utf8");
function loadComponent(artworkSnapshot = { active: false, outcome: "idle", total: 0, processed: 0, found: 0, onlineFound: 0, failed: 0 }) {
  let cursor = 0;
  const slots = [];
  const react = { useState(initial) { const i = cursor++; slots[i] ??= typeof initial === "function" ? initial() : initial; return [slots[i], value => { slots[i] = typeof value === "function" ? value(slots[i]) : value; }]; }, useSyncExternalStore(_subscribe, getSnapshot) { return getSnapshot(); } };
  const js = ts.transpileModule(source, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
  const exports = {};
  new Function("require", "exports", js)(id => id === "react" ? { ...react, default: react } : id === "react/jsx-runtime" ? jsx : id === "lucide-react" ? icons : id === "react-i18next" ? { useTranslation: () => ({ t: key => key }) } : id === "../utils/localArtworkSession" ? { localArtworkEnrichment: { subscribe: () => () => {}, getSnapshot: () => artworkSnapshot } } : (() => { throw new Error(id); })(), exports);
  return { render(props) { cursor = 0; return exports.SqliteSourcesSettings(props); } };
}
function all(node) { return !node || typeof node !== "object" ? [] : [node, ...(Array.isArray(node.props?.children) ? node.props.children.flatMap(all) : all(node.props?.children))]; }
function text(node) { return typeof node === "string" || typeof node === "number" ? String(node) : Array.isArray(node) ? node.map(text).join("") : node?.props ? text(node.props.children) : ""; }
function button(tree, label) { const found = all(tree).find(node => node.type === "button" && text(node) === label); assert.ok(found, `missing button ${label}`); return found; }

test("SQLite settings lists the catalog source and delegates all management to SQLite callbacks", async () => {
  const view = loadComponent();
  const calls = [];
  const props = { sources: [{ id: 11, displayName: "Fixture NAS", generation: 8, trackCount: 26 }], progress: null, busy: false,
    onAddFolder: () => calls.push(["add"]), onVerify: async id => { calls.push(["verify", id]); return true; },
    onReindex: async id => calls.push(["reindex", id]), onRemove: async id => calls.push(["remove", id]) };
  let tree = view.render(props);
  assert.match(text(tree), /Fixture NAS/);
  assert.match(text(tree), /26 sqliteLibrary.sourceTrackCount/);
  button(tree, "sqliteLibrary.addFolder").props.onClick();
  button(tree, "sqliteLibrary.verifyChanges").props.onClick();
  button(tree, "sqliteLibrary.reindexShort").props.onClick();
  button(tree, "sqliteLibrary.removeShort").props.onClick();
  tree = view.render(props);
  const dialog = all(tree).find(node => node.props?.role === "alertdialog");
  assert.ok(dialog, "removal requires explicit confirmation");
  const confirm = all(dialog).find(node => node.type === "button" && text(node) === "sqliteLibrary.removeShort");
  await confirm.props.onClick();
  await Promise.resolve();
  assert.deepEqual(calls, [["add"], ["verify", "sqlite-11"], ["reindex", "sqlite-11"], ["remove", "sqlite-11"]]);
});

test("SQLite Settings exposes the source page and does not point Local management at folders.json or MPD", () => {
  const modal = readFileSync(new URL("../components/SettingsModal.tsx", import.meta.url), "utf8");
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(modal, /disabledPages=\{fullModeEnabled\(config\) \? \[\] : \["experimental"\]\}/);
  assert.match(modal, /sqliteExperimental \? <SqliteSourcesSettings/);
  assert.match(modal, /onVerify=\{sourceId => onSqliteVerifySource/);
  assert.match(modal, /onRemove=\{sourceId => onSqliteRemoveSource/);
  assert.match(app, /sqliteSources=\{sqliteSources\}/);
  assert.match(app, /onSqliteAddFolder=\{addSqliteFolder\}/);
  assert.match(app, /onSqliteVerifySource=\{verifySqliteSource\}/);
  assert.match(app, /onSqliteReindexSource=\{reindexSqliteSource\}/);
  assert.match(app, /onSqliteRemoveSource=\{removeSqliteSource\}/);
});

test("SQLite artwork status distinguishes active counts, partial failures, and cancellation", () => {
  const props = { sources: [], progress: null, busy: false, onAddFolder() {}, onVerify() {}, onReindex() {}, onRemove() {} };
  const active = loadComponent({ active: true, outcome: "running", total: 12, processed: 7, found: 2, onlineFound: 1, failed: 1 });
  const activeText = text(active.render(props));
  assert.match(activeText, /settings\.onlineArtworkProgress/);
  assert.match(activeText, /settings\.onlineArtworkFound/);
  assert.match(activeText, /settings\.onlineArtworkFailed/);

  for (const [outcome, label] of [["partial", "settings.onlineArtworkPartial"], ["cancelled", "settings.onlineArtworkCancelled"]]) {
    const terminal = loadComponent({ active: false, outcome, total: 12, processed: 12, found: 3, onlineFound: 2, failed: outcome === "partial" ? 1 : 0 });
    assert.match(text(terminal.render(props)), new RegExp(label.replaceAll(".", "\\.")));
  }
});
