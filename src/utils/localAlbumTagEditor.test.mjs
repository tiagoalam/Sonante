import test from "node:test";
import assert from "node:assert/strict";

import {
  analyzeLocalAlbumTagForm,
  buildLocalAlbumTagBatchPreflightRequest,
  clearMixedLocalAlbumTagField,
  createLocalAlbumTagEditorDraft,
  editLocalAlbumTagField,
  localAlbumTagPreviewView,
  createLocalAlbumTagRequestGuard,
  resetLocalAlbumTagField,
} from "./localAlbumTagEditor.ts";

const tags = (revision) => ({
  title: null,
  artist: null,
  album: "Album",
  album_artist: null,
  genre: null,
  date: null,
  track_number: null,
  track_total: null,
  disc_number: null,
  disc_total: null,
  format: "flac",
  readable: true,
  writable: true,
  editable: true,
  unsupported_reason: null,
  revision,
});

const inspection = (overrides = {}) => ({
  album_id: "album-id",
  tracks: [
    { uri: "Music/a.flac", kind: "physical", tags: tags("rev-a"), issue: null },
    { uri: "Music/image.cue/track0001", kind: "cue_virtual", tags: null, issue: null },
  ],
  fields: {
    album: { state: "common", value: "Album" },
    album_artist: { state: "absent", value: null },
    date: { state: "mixed", value: null },
    genre: { state: "mixed", value: null },
  },
  physical_track_count: 1,
  cue_virtual_count: 1,
  inspection_complete: true,
  ...overrides,
});

test("common fields distinguish unchanged, set, and explicit remove", () => {
  const initial = createLocalAlbumTagEditorDraft(inspection());
  assert.deepEqual(analyzeLocalAlbumTagForm(initial).update.album, { action: "unchanged" });
  assert.equal(analyzeLocalAlbumTagForm(initial).dirty, false);

  const changed = editLocalAlbumTagField(initial, "album", "New Album");
  assert.deepEqual(analyzeLocalAlbumTagForm(changed).update.album, {
    action: "set",
    value: "New Album",
  });
  assert.equal(analyzeLocalAlbumTagForm(changed).dirty, true);

  const cleared = editLocalAlbumTagField(initial, "album", "");
  assert.deepEqual(analyzeLocalAlbumTagForm(cleared).update.album, { action: "remove" });
});

test("absent fields remain unchanged until a value is entered", () => {
  const initial = createLocalAlbumTagEditorDraft(inspection());
  assert.deepEqual(analyzeLocalAlbumTagForm(initial).update.album_artist, {
    action: "unchanged",
  });
  const changed = editLocalAlbumTagField(initial, "album_artist", "Artist");
  assert.deepEqual(analyzeLocalAlbumTagForm(changed).update.album_artist, {
    action: "set",
    value: "Artist",
  });
});

test("mixed visually empty stays unchanged until explicit set or clear for all", () => {
  const initial = createLocalAlbumTagEditorDraft(inspection());
  assert.equal(initial.fields.genre.value, "");
  assert.deepEqual(analyzeLocalAlbumTagForm(initial).update.genre, { action: "unchanged" });

  const emptyEdit = editLocalAlbumTagField(initial, "genre", "");
  assert.deepEqual(analyzeLocalAlbumTagForm(emptyEdit).update.genre, { action: "unchanged" });

  const changed = editLocalAlbumTagField(initial, "genre", "MPB");
  assert.deepEqual(analyzeLocalAlbumTagForm(changed).update.genre, {
    action: "set",
    value: "MPB",
  });

  const cleared = clearMixedLocalAlbumTagField(initial, "genre");
  assert.deepEqual(analyzeLocalAlbumTagForm(cleared).update.genre, { action: "remove" });
  assert.equal(analyzeLocalAlbumTagForm(cleared).dirty, true);
});

test("expected snapshot comes only from inspection membership and revisions", () => {
  const draft = createLocalAlbumTagEditorDraft(inspection());
  assert.deepEqual(buildLocalAlbumTagBatchPreflightRequest(draft), {
    album_id: "album-id",
    tracks: [
      { uri: "Music/a.flac", kind: "physical", revision: "rev-a" },
      { uri: "Music/image.cue/track0001", kind: "cue_virtual", revision: null },
    ],
    update: {
      album: { action: "unchanged" },
      album_artist: { action: "unchanged" },
      date: { action: "unchanged" },
      genre: { action: "unchanged" },
    },
  });
});

test("indeterminate and not applicable summaries block editing and preflight", () => {
  for (const state of ["indeterminate", "not_applicable"]) {
    const draft = createLocalAlbumTagEditorDraft(inspection({
      fields: {
        ...inspection().fields,
        album: { state, value: null },
      },
    }));
    assert.equal(analyzeLocalAlbumTagForm(draft).blocked, true);
  }
});

test("spaces and Unicode are preserved without normalization", () => {
  const initial = createLocalAlbumTagEditorDraft(inspection());
  const value = "  Álbum Único  ";
  const edited = editLocalAlbumTagField(initial, "album", value);
  assert.deepEqual(analyzeLocalAlbumTagForm(edited).update.album, {
    action: "set",
    value,
  });
});

const physicalInspection = (overrides = {}) => inspection({
  tracks: [
    { uri: "Music/a.flac", kind: "physical", tags: tags("rev-a"), issue: null },
    { uri: "Music/b.flac", kind: "physical", tags: { ...tags("rev-b"), genre: "Jazz" }, issue: null },
  ],
  physical_track_count: 2, cue_virtual_count: 0, ...overrides,
});
const unchanged = () => Object.fromEntries(Object.keys(tags("rev")).filter(key =>
  ["title", "artist", "album", "album_artist", "genre", "date", "track_number", "track_total", "disc_number", "disc_total"].includes(key)
).map(key => [key, { action: "unchanged" }]));
const plan = () => ({
  album_id: "album-id", applicable: true, blocked_reason: null,
  tracks: [
    { uri: "Music/a.flac", expected_revision: "rev-a", update: { ...unchanged(), genre: { action: "set", value: "  MPB Única  " } }, needs_write: true, issue: null },
    { uri: "Music/b.flac", expected_revision: "rev-b", update: unchanged(), needs_write: false, issue: null },
  ], change_count: 1, unchanged_count: 1,
});

test("CUE, incomplete, missing tags and uneditable formats block the entire album", () => {
  assert.equal(analyzeLocalAlbumTagForm(createLocalAlbumTagEditorDraft(inspection())).blockedReason, "cue");
  for (const overrides of [{ inspection_complete: false }, { physical_track_count: 0 }, {
    tracks: [{ ...physicalInspection().tracks[0], tags: null, issue: { code: "read_failed" } }],
  }]) assert.equal(analyzeLocalAlbumTagForm(createLocalAlbumTagEditorDraft(physicalInspection(overrides))).blockedReason, "inspection");
  for (const field of ["editable", "writable", "readable"]) {
    const snapshot = physicalInspection(); snapshot.tracks[1].tags[field] = false;
    snapshot.tracks[1].tags.format = "mp3";
    assert.equal(analyzeLocalAlbumTagForm(createLocalAlbumTagEditorDraft(snapshot)).blockedReason, "format");
  }
  assert.equal(analyzeLocalAlbumTagForm(createLocalAlbumTagEditorDraft(physicalInspection())).blocked, false);
});

test("reset restores mixed values; request touches only four album fields", () => {
  const initial = createLocalAlbumTagEditorDraft(physicalInspection());
  const cleared = clearMixedLocalAlbumTagField(initial, "genre");
  assert.equal(analyzeLocalAlbumTagForm(resetLocalAlbumTagField(cleared, "genre")).dirty, false);
  assert.deepEqual(Object.keys(buildLocalAlbumTagBatchPreflightRequest(cleared).update), ["album", "album_artist", "date", "genre"]);
  assert.equal(initial.fields.genre.clearAll, false);
});

test("per-file preview uses backend actions and inspected previous values, including unchanged files", () => {
  const view = localAlbumTagPreviewView(plan(), physicalInspection());
  assert.deepEqual(view.files, [
    { uri: "Music/a.flac", needsWrite: true, blocked: false, changes: [{ field: "genre", before: null, proposed: "  MPB Única  " }] },
    { uri: "Music/b.flac", needsWrite: false, blocked: false, changes: [] },
  ]);
  assert.equal(view.changeCount, 1); assert.equal(view.unchangedCount, 1); assert.equal(view.canApply, false);
  const removal = plan(); removal.tracks[0].update = { ...unchanged(), album: { action: "remove" } };
  assert.deepEqual(localAlbumTagPreviewView(removal, physicalInspection()).files[0].changes,
    [{ field: "album", before: "Album", proposed: null }]);
  removal.applicable = false; removal.blocked_reason = "track_inspection_failed";
  assert.equal(localAlbumTagPreviewView(removal, physicalInspection()).canApply, false);
});

test("preview rejects unknown or duplicate URIs, changed revisions, counts and individual field mutations", () => {
  for (const mutate of [
    p => p.album_id = "another", p => p.tracks[0].uri = "unknown",
    p => p.tracks[0].expected_revision = "stale", p => p.tracks.push(p.tracks[0]),
    p => p.tracks.pop(), p => p.change_count++, p => p.unchanged_count++,
    p => p.tracks[0].needs_write = false,
    p => p.tracks[0].update.title = { action: "set", value: "not an album field" },
  ]) { const p = plan(); mutate(p); assert.throws(() => localAlbumTagPreviewView(p, physicalInspection())); }
});

test("request guard rejects results after edits, reload, album/catalog changes, blocks and unmount", () => {
  const guard = createLocalAlbumTagRequestGuard(), catalog = {};
  guard.syncContext("a", catalog, true);
  let token = guard.capture(); assert.equal(guard.isCurrent(token), true);
  guard.invalidate(); assert.equal(guard.isCurrent(token), false);
  token = guard.capture(); guard.capture(); assert.equal(guard.isCurrent(token), false);
  for (const [album, version, available] of [["b", catalog, true], ["b", {}, true], ["b", {}, false], ["b", {}, true]]) {
    token = guard.capture(); guard.syncContext(album, version, available);
    assert.equal(guard.isCurrent(token), false);
  }
  guard.syncContext("a", catalog, true); token = guard.capture(); guard.invalidate();
  assert.equal(guard.isCurrent(token), false);
});

// Execute the real modal and handlers with hook/IPC substitutes, without Tauri or MPD.
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "lucide-react";
import * as editor from "./localAlbumTagEditor.ts";

const modalSource = readFileSync(new URL("../components/LocalAlbumTagEditorModal.tsx", import.meta.url), "utf8");
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
function modalFixture(service = {}, initialProps = {}) {
  const previous = { document: globalThis.document, window: globalThis.window, HTMLElement: globalThis.HTMLElement };
  globalThis.HTMLElement = class {};
  globalThis.document = { activeElement: null, addEventListener() {}, removeEventListener() {} };
  const confirmations = []; let confirm = true, index = 0, closes = 0;
  globalThis.window = { confirm(message) { confirmations.push(message); return confirm; } };
  const slots = [], pending = [], calls = { inspect: [], preflight: [] };
  const hooks = { ...React,
    useState(initial) { const key = index++; if (!(key in slots)) slots[key] = typeof initial === "function" ? initial() : initial;
      return [slots[key], value => { slots[key] = typeof value === "function" ? value(slots[key]) : value; }]; },
    useRef(initial) { const key = index++; return slots[key] ??= { current: initial }; },
    useMemo(fn) { index++; return fn(); },
    useEffect(effect, deps) { const key = index++, old = slots[key];
      if (!old || deps.some((value, i) => !Object.is(value, old.deps[i]))) pending.push(() => {
        old?.cleanup?.(); slots[key] = { deps, cleanup: effect() };
      }); },
  };
  const dependencies = {
    react: { ...hooks, default: hooks, __esModule: true }, "react/jsx-runtime": jsx, "lucide-react": icons,
    "react-i18next": { useTranslation: () => ({ t: translate }) },
    "../utils/localAlbumTagEditor": editor,
    "../services/localTags": { localTagsService: {
      getAlbumInspection(id) { calls.inspect.push(id); return service.inspect?.(id) ?? Promise.resolve(physicalInspection()); },
      preflightAlbumBatch(request) { calls.preflight.push(request); return service.preflight?.(request) ?? Promise.resolve(plan()); },
    } },
  };
  const js = ts.transpileModule(modalSource, { compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  const exports = {};
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, `Unexpected import ${id}`); return dependencies[id]; }, exports);
  const props = { albumId: "album-id", displayName: "Album", available: true, catalogVersion: {}, isAvailable: () => true, onClose() { closes++; }, ...initialProps };
  const f = {
    calls, props, confirmations, tree: null,
    render() { index = 0; f.tree = exports.LocalAlbumTagEditorModal(props); return f.tree; },
    effects() { pending.splice(0).forEach(run => run()); },
    input(field) { return findNode(f.tree, n => n.type === "input" && n.props.id === `local-album-tag-${field}`); },
    button(key) { return findNode(f.tree, n => n.type === "button" && content(n) === key); },
    async edit(value) { f.input("genre").props.onChange({ target: { value } }); f.render(); f.effects(); },
    async preview() { await findNode(f.tree, n => n.type === "form").props.onSubmit({ preventDefault() {} }); f.render(); f.effects(); },
    confirm(value) { confirm = value; }, get closes() { return closes; },
    dispose() { slots.forEach(slot => slot?.cleanup?.()); Object.assign(globalThis, previous); },
  };
  f.render(); f.effects(); return f;
}
const translate = key => key;
function nodes(tree) {
  if (!React.isValidElement(tree)) return Array.isArray(tree) ? tree.flatMap(nodes) : [];
  return [tree, ...nodes(tree.props.children)];
}
function content(tree) { return React.isValidElement(tree) ? content(tree.props.children) : Array.isArray(tree) ? tree.map(content).join("") : typeof tree === "string" || typeof tree === "number" ? String(tree) : ""; }
function findNode(tree, predicate) { const node = nodes(tree).find(predicate); assert.ok(node, "Expected control"); return node; }
async function ready(service) { const f = modalFixture(service); await flush(); f.render(); f.effects(); return f; }

test("modal shows backend per-file preview and editing immediately invalidates it", async () => {
  const f = await ready();
  try {
    await f.edit("MPB"); await f.preview();
    assert.match(content(f.tree), /Music\/a.flac/); assert.match(content(f.tree), /Music\/b.flac/);
    assert.match(content(f.tree), /albumTagEditor.before/); assert.match(content(f.tree), /  MPB Única  /);
    assert.equal(f.calls.preflight[0].tracks.length, 2);
    await f.edit("Other"); assert.doesNotMatch(content(f.tree), /Music\/a.flac/);
    assert.equal(f.button("albumTagEditor.preview").props.disabled, false);
  } finally { f.dispose(); }
});

test("modal blocks CUE and noneditable formats from inspection, without partial preflight", async () => {
  for (const snapshot of [inspection(), physicalInspection({ tracks: [
    { ...physicalInspection().tracks[0], tags: { ...tags("rev-a"), format: "mp3", editable: false, writable: false } },
  ] })]) {
    const f = await ready({ inspect: async () => snapshot });
    try {
      assert.equal(f.input("genre").props.disabled, true);
      assert.match(content(f.tree), snapshot.cue_virtual_count ? /albumTagEditor.cueBlocked/ : /albumTagEditor.formatBlocked/);
      await f.preview(); assert.equal(f.calls.preflight.length, 0);
    } finally { f.dispose(); }
  }
});

test("catalog invalidation hides completed previews and preserves edits until confirmed Reload", async () => {
  const f = await ready();
  try {
    await f.edit("Kept"); await f.preview();
    f.props.catalogVersion = {}; f.render(); // Hidden before effects run.
    assert.doesNotMatch(content(f.tree), /Music\/a.flac/);
    f.effects(); f.render(); assert.equal(f.input("genre").props.value, "Kept");
    assert.equal(f.button("albumTagEditor.preview").props.disabled, true);
    f.confirm(false); f.button("albumTagEditor.reload").props.onClick();
    assert.equal(f.calls.inspect.length, 1); assert.equal(f.input("genre").props.value, "Kept");
    f.confirm(true); f.button("albumTagEditor.reload").props.onClick(); await flush(); f.render();
    assert.equal(f.calls.inspect.length, 2); assert.equal(f.input("genre").props.value, "");
    assert.deepEqual(f.confirmations, ["albumTagEditor.reloadConfirm", "albumTagEditor.reloadConfirm"]);
  } finally { f.dispose(); }
});

test("late preflight responses are ignored after edit, catalog, album, block or disposal", async () => {
  for (const action of ["edit", "catalog", "album", "block", "dispose"]) {
    const delayed = deferred(), f = await ready({ preflight: () => delayed.promise }); let disposed = false;
    try {
      await f.edit("Kept"); const request = findNode(f.tree, n => n.type === "form").props.onSubmit({ preventDefault() {} }); await flush();
      if (action === "edit") await f.edit("New edit");
      if (action === "catalog") f.props.catalogVersion = {};
      if (action === "album") f.props.albumId = "other-album";
      if (action === "block") f.props.available = false;
      if (action === "dispose") { f.dispose(); disposed = true; }
      else { f.render(); f.effects(); }
      delayed.resolve(plan()); await request;
      if (!disposed) { f.render(); f.effects(); }
      assert.doesNotMatch(content(f.tree), /Music\/a.flac/);
      if (action === "edit") assert.equal(f.input("genre").props.value, "New edit");
      if (action === "block") assert.equal(f.input("genre").props.value, "Kept");
    } finally { if (!disposed) f.dispose(); }
  }
});

test("conflicts and operational errors preserve drafts and closing requires discard confirmation", async () => {
  for (const code of ["revision_conflict", "membership_changed", "invalid_snapshot", "album_catalog_failure"]) {
    const f = await ready({ preflight: async () => { throw { code }; } });
    try {
      await f.edit("Kept"); await f.preview(); assert.equal(f.input("genre").props.value, "Kept");
      assert.doesNotMatch(content(f.tree), /Music\/a.flac/);
      f.confirm(false); f.button("albumTagEditor.cancel").props.onClick(); assert.equal(f.closes, 0);
      f.confirm(true); f.button("albumTagEditor.cancel").props.onClick(); assert.equal(f.closes, 1);
      assert.deepEqual(f.confirmations, ["albumTagEditor.discardConfirm", "albumTagEditor.discardConfirm"]);
    } finally { f.dispose(); }
  }
});

test("failed Reload keeps edits and invalidates a previous preview; availability is checked before IPC", async () => {
  let attempts = 0;
  const f = await ready({ inspect: async () => { if (++attempts > 1) throw new Error("offline"); return physicalInspection(); } });
  try {
    await f.edit("Kept"); await f.preview(); f.button("albumTagEditor.reload").props.onClick(); await flush(); f.render();
    assert.equal(f.input("genre").props.value, "Kept"); assert.doesNotMatch(content(f.tree), /Music\/a.flac/);
    assert.equal(f.button("albumTagEditor.preview").props.disabled, true);
    f.props.isAvailable = () => false; f.render(); f.button("albumTagEditor.reload").props.onClick();
    assert.equal(f.calls.inspect.length, 2);
  } finally { f.dispose(); }
  const g = await ready();
  try { await g.edit("Kept"); g.props.isAvailable = () => false; g.render(); await g.preview(); assert.equal(g.calls.preflight.length, 0); }
  finally { g.dispose(); }
});

test("late inspection cannot replace the draft after a block; unblocking requires fresh inspection", async () => {
  const delayed = deferred(), f = modalFixture({ inspect: () => delayed.promise });
  try {
    f.props.available = false; f.render(); f.effects();
    delayed.resolve(physicalInspection()); await flush(); f.render();
    assert.equal(nodes(f.tree).some(n => n.type === "input"), false);
    f.props.available = true; f.render(); f.effects(); f.render();
    assert.equal(f.calls.inspect.length, 1);
  } finally { f.dispose(); }
});

test("integration retains public navigation contracts, localized strings and read-only album IPC", () => {
  const browser = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  for (const key of ["playerTarget", "onPlayerNavigationBack", "importPreview", "catalogRefreshRevision", "configuredLocalFolders", "createAlbumTracksLoader", "getLocalAlbumTracks"])
    assert.ok(browser.includes(key), key);
  assert.match(browser, /disabled=\{!catalogConfirmed\}/);
  assert.match(browser, /current\.saveBlocked/); assert.match(browser, /current\.mpdBusy/);
  assert.doesNotMatch(modalSource, /apply_local|\.update\(|Apply/);
  const keys = [...modalSource.matchAll(/"(albumTagEditor\.[^"]+)"/g)].map(m => m[1]);
  for (const locale of ["en", "pt-BR"]) {
    const resource = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    for (const key of keys) assert.equal(typeof key.split(".").reduce((obj, part) => obj?.[part], resource), "string", `${locale}: ${key}`);
    for (const field of ["album", "album_artist", "date", "genre"]) assert.equal(typeof resource.tagEditor.fields[field], "string");
  }
});

test("a preflight inspection failure is distinct from an unchanged file", () => {
  const p = plan(); p.applicable = false; p.blocked_reason = "track_inspection_failed";
  p.tracks[1].issue = { code: "read_failed" }; p.unchanged_count = 0;
  const view = localAlbumTagPreviewView(p, physicalInspection());
  assert.equal(view.files[1].blocked, true); assert.equal(view.unchangedCount, 0);
  assert.equal(view.applicable, false);
});

test("an initially unavailable catalog causes no inspection and a late rejection cannot replace newer edits", async () => {
  const f = modalFixture({}, { available: false, isAvailable: () => false });
  try {
    await flush(); f.render();
    f.button("albumTagEditor.reload").props.onClick(); assert.equal(f.calls.inspect.length, 0);
  } finally { f.dispose(); }
  const delayed = deferred(), g = await ready({ preflight: () => delayed.promise });
  try {
    await g.edit("First"); const request = findNode(g.tree, n => n.type === "form").props.onSubmit({ preventDefault() {} });
    await g.edit("Latest"); delayed.reject({ code: "revision_conflict" }); await request; g.render();
    assert.equal(g.input("genre").props.value, "Latest");
    assert.equal(g.button("albumTagEditor.preview").props.disabled, false);
    assert.doesNotMatch(content(g.tree), /albumTagEditor.stale/);
  } finally { g.dispose(); }
});
