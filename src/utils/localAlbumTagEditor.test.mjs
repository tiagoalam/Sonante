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
  assert.equal(view.changeCount, 1); assert.equal(view.unchangedCount, 1); assert.equal(view.canApply, true);
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
  const slots = [], pending = [], calls = { inspect: [], preflight: [], apply: [], finished: [] };
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
      applyAlbumBatch(request) { calls.apply.push(request); return service.apply?.(request) ?? Promise.resolve(batchResult()); },
    } },
  };
  const js = ts.transpileModule(modalSource, { compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  const exports = {};
  new Function("require", "exports", js)(id => { assert.ok(id in dependencies, `Unexpected import ${id}`); return dependencies[id]; }, exports);
  const props = { albumId: "album-id", displayName: "Album", available: true, catalogVersion: {}, isAvailable: () => true, onClose() { closes++; }, onBatchFinished(result) { calls.finished.push(result); }, ...initialProps };
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
const translate = (key, values) => key + (values ? JSON.stringify(values) : "");
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

test("integration retains public navigation contracts, localized strings and album IPC contracts", () => {
  const browser = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  for (const key of ["playerTarget", "onPlayerNavigationBack", "importPreview", "catalogRefreshRevision", "configuredLocalFolders", "createAlbumTracksLoader", "getLocalAlbumTracks"])
    assert.ok(browser.includes(key), key);
  assert.match(browser, /!albumTagEditingAvailable\(albumId\)/);
  assert.match(browser, /current\.saveBlocked/); assert.match(browser, /current\.mpdBusy/);
  assert.match(modalSource, /applyAlbumBatch\(approved\.request\)/);
  assert.doesNotMatch(modalSource, /applyAlbumBatch\([^)]*plan/);
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

const batchResult = (overrides = {}) => ({
  album_id: "album-id", applicable: true, blocked_reason: null,
  tracks: [
    { uri: "Music/a.flac", expected_revision: "rev-a", state: "committed", tags: tags("rev-new"), error: null,
      mpd_refresh: { state: "accepted", job_id: 17, error_code: null } },
    { uri: "Music/b.flac", expected_revision: "rev-b", state: "unchanged", tags: null, error: null,
      mpd_refresh: { state: "not_needed", job_id: null, error_code: null } },
  ], committed_count: 1, unchanged_count: 1, failed_count: 0, not_attempted_count: 0, partial_commit: false,
  ...overrides,
});
const applyHandler = f => f.button("albumTagEditor.apply").props.onClick;
function assertFooter(f, rightButtons) {
  const footer = findNode(f.tree, n => n.type === "footer");
  const buttons = nodes(footer).filter(n => n.type === "button");
  assert.deepEqual(buttons.map(content), ["albumTagEditor.reload", ...rightButtons]);
  return buttons;
}
const assertClosedStage = f => assertFooter(f, ["albumTagEditor.closeButton"]);
async function reviewed(service, props) {
  const f = modalFixture(service, props); await flush(); f.render(); f.effects();
  await f.edit("MPB"); await f.preview(); return f;
}

// Tests continue to invoke real modal handlers, with the write IPC fully substituted.
test("footer swaps submit Preview for button Apply only after valid preflight, without sending automatically", async () => {
  const delayed = deferred(), f = await ready({ preflight: () => delayed.promise });
  try {
    const initial = assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
    assert.equal(initial[0].props.type, "button");
    assert.equal(initial[1].props.type, "button");
    assert.equal(initial[2].props.type, "submit"); assert.equal(initial[2].props.disabled, true);
    await f.edit("MPB"); assert.equal(f.button("albumTagEditor.preview").props.disabled, false);
    const preview = f.preview(); f.render();
    assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.previewing"]);
    assert.equal(f.button("albumTagEditor.previewing").props.disabled, true);
    delayed.resolve(plan()); await preview;
    const reviewed = assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.apply"]);
    assert.equal(reviewed[2].props.type, "button"); assert.equal(reviewed[2].props.disabled, false);
    assert.equal(f.calls.apply.length, 0); assert.equal(f.confirmations.length, 0);
    f.confirm(false); applyHandler(f)(); f.render();
    assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.apply"]);
    assert.equal(f.calls.apply.length, 0);
    await f.edit("New"); assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
    await f.preview(); assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.apply"]);
    f.confirm(true); f.button("albumTagEditor.reload").props.onClick(); await flush(); f.render();
    assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
    assert.equal(f.button("albumTagEditor.preview").props.disabled, true);
    assert.equal(f.calls.apply.length, 0); assert.equal(f.calls.preflight.length, 2);
  } finally { f.dispose(); }
});

test("footer immediately returns to blocked Preview when album, catalog or availability invalidates authorization", async () => {
  for (const change of [f => { f.props.albumId = "other"; }, f => { f.props.catalogVersion = {}; }, f => { f.props.available = false; }]) {
    const f = await reviewed();
    try {
      change(f); f.render();
      assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
      assert.equal(f.button("albumTagEditor.preview").props.disabled, true);
      f.effects(); f.render(); assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
      assert.equal(f.calls.apply.length, 0); assert.equal(f.confirmations.length, 0);
    } finally { f.dispose(); }
  }
});

test("footer Close replaces both write actions after success or Apply error and retains discard confirmation", async () => {
  for (const service of [{}, { apply: async () => { throw { code: "revision_conflict" }; } }, { apply: async () => { throw new Error("IPC interrupted"); } }]) {
    const f = await reviewed(service);
    try {
      applyHandler(f)(); await flush(); f.render();
      const buttons = assertClosedStage(f);
      assert.equal(buttons[1].props.type, "button"); assert.equal(buttons[1].props.disabled, false);
      f.confirm(false); buttons[1].props.onClick(); assert.equal(f.closes, 0);
      f.confirm(true); buttons[1].props.onClick(); assert.equal(f.closes, 1);
      assert.deepEqual(f.confirmations.slice(-2), ["albumTagEditor.resultCloseConfirm", "albumTagEditor.resultCloseConfirm"]);
      assert.equal(f.calls.apply.length, 1);
    } finally { f.dispose(); }
  }
});

test("Apply requires explicit confirmation and sends the exact immutable preflight request, never the plan", async () => {
  const f = await reviewed();
  try {
    const request = f.calls.preflight[0]; assert.ok(Object.isFrozen(request));
    assert.ok(Object.isFrozen(request.update.genre)); assert.ok(Object.isFrozen(request.tracks[0]));
    f.confirm(false); applyHandler(f)(); await flush(); f.render();
    assert.equal(f.calls.apply.length, 0);
    f.confirm(true); applyHandler(f)(); await flush(); f.render();
    assert.equal(f.calls.apply.length, 1); assert.equal(f.calls.apply[0], request);
    assert.ok(f.confirmations[0].startsWith("albumTagEditor.applyConfirm"));
    assert.deepEqual(Object.keys(f.calls.apply[0]), ["album_id", "tracks", "update"]);
    assert.match(content(f.tree), /albumTagEditor.outcomes.committed/);
    assert.match(content(f.tree), /albumTagEditor.refresh.accepted\{"jobId":17\}/);
    assert.equal(f.calls.finished.length, 1);
  } finally { f.dispose(); }
});

test("Apply refuses absent, blocked, zero-change and obsolete previews or an unavailable catalog", async () => {
  const initial = await ready();
  try { assertFooter(initial, ["albumTagEditor.cancel", "albumTagEditor.preview"]); assert.equal(initial.calls.apply.length, 0); } finally { initial.dispose(); }
  const zero = plan(); zero.tracks[0].update = unchanged(); zero.tracks[0].needs_write = false; zero.change_count = 0; zero.unchanged_count = 2;
  const blocked = plan(); blocked.applicable = false; blocked.blocked_reason = "track_inspection_failed";
  for (const p of [zero, blocked]) {
    const f = await reviewed({ preflight: async () => p });
    try { assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]); assert.equal(f.calls.apply.length, 0); }
    finally { f.dispose(); }
  }
  for (const action of ["edit", "catalog", "unavailable"]) {
    const f = await reviewed();
    try {
      const click = applyHandler(f);
      if (action === "edit") await f.edit("New");
      if (action === "catalog") { f.props.catalogVersion = {}; f.render(); }
      if (action === "unavailable") { f.props.isAvailable = () => false; f.render(); }
      if (action === "unavailable") applyHandler(f)();
      else click();
      assert.equal(f.calls.apply.length, 0);
    } finally { f.dispose(); }
  }
});

test("availability and request generation are rechecked after confirmation", async () => {
  const f = await reviewed();
  try {
    globalThis.window.confirm = () => { f.props.available = false; f.render(); return true; };
    applyHandler(f)(); assert.equal(f.calls.apply.length, 0);
  } finally { f.dispose(); }
});

test("synchronous double clicks send once and Apply blocks edits, Reload and every close path", async () => {
  const delayed = deferred(), f = await reviewed({ apply: () => delayed.promise });
  try {
    const click = applyHandler(f), reload = f.button("albumTagEditor.reload").props.onClick;
    const cancel = f.button("albumTagEditor.cancel").props.onClick, edit = f.input("genre").props.onChange;
    click(); click(); assert.equal(f.calls.apply.length, 1);
    // Old handlers from before React rerenders must also respect the synchronous lock.
    edit({ target: { value: "Must not change" } }); reload(); cancel();
    f.render(); f.effects();
    assert.equal(f.input("genre").props.value, "MPB"); assert.equal(f.input("genre").props.disabled, true);
    assert.equal(f.closes, 0); assert.equal(f.calls.inspect.length, 1);
    assert.equal(f.button("albumTagEditor.reload").props.disabled, true);
    assert.equal(f.button("albumTagEditor.cancel").props.disabled, true);
    const buttons = assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.applyingButton"]);
    assert.equal(buttons[2].props.type, "button"); assert.equal(buttons[2].props.disabled, true);
    assert.ok(nodes(buttons[2]).some(n => n.props.className === "animate-spin"));
    delayed.resolve(batchResult()); await flush(); f.render();
    click(); assert.equal(f.calls.apply.length, 1);
    assertClosedStage(f);
  } finally { f.dispose(); }
});

test("conflicts before writes retain edits and require a fresh inspection and preview", async () => {
  const f = await reviewed({ apply: async () => { throw { code: "revision_conflict" }; } });
  try {
    const click = applyHandler(f); click(); await flush(); f.render();
    assert.match(content(f.tree), /albumTagEditor.applyConflict/); assert.equal(f.input("genre").props.value, "MPB");
    assertClosedStage(f);
    click(); assert.equal(f.calls.apply.length, 1);
    f.confirm(false); f.button("albumTagEditor.reload").props.onClick(); assert.equal(f.calls.inspect.length, 1);
    f.confirm(true); f.button("albumTagEditor.reload").props.onClick(); await flush(); f.render();
    assert.equal(f.calls.inspect.length, 2); assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
    await f.edit("New"); await f.preview(); assert.equal(f.button("albumTagEditor.apply").props.disabled, false);
  } finally { f.dispose(); }
});

test("partial commits expose every outcome, counts and post-write warnings separately from refresh", async () => {
  const snapshot = physicalInspection();
  snapshot.tracks.push({ uri: "Music/c.flac", kind: "physical", tags: tags("rev-c"), issue: null },
    { uri: "Music/d.flac", kind: "physical", tags: tags("rev-d"), issue: null }); snapshot.physical_track_count = 4;
  const p = plan();
  for (const [uri, revision] of [["Music/c.flac", "rev-c"], ["Music/d.flac", "rev-d"]]) {
    p.tracks.push({ uri, expected_revision: revision, update: { ...unchanged(), genre: { action: "set", value: "MPB" } }, needs_write: true, issue: null });
  }
  p.change_count = 3;
  const r = batchResult(); r.tracks[0].tags = null;
  r.tracks[0].error = { code: "durability_failed", committed: true };
  r.tracks[0].mpd_refresh = { state: "failed", job_id: null, error_code: "mpd_unavailable" };
  r.tracks.push({ uri: "Music/c.flac", expected_revision: "rev-c", state: "failed", tags: null, error: { code: "permission_denied", committed: false }, mpd_refresh: { state: "not_needed", job_id: null, error_code: null } },
    { uri: "Music/d.flac", expected_revision: "rev-d", state: "not_attempted", tags: null, error: null, mpd_refresh: { state: "not_needed", job_id: null, error_code: null } });
  r.failed_count = 1; r.not_attempted_count = 1; r.partial_commit = true;
  const f = await reviewed({ inspect: async () => snapshot, preflight: async () => p, apply: async () => r });
  try {
    applyHandler(f)(); await flush(); f.render();
    for (const state of ["committed", "unchanged", "failed", "not_attempted"]) assert.match(content(f.tree), new RegExp(`albumTagEditor.outcomes.${state}`));
    assert.match(content(f.tree), /"committed":1,"unchanged":1,"failed":1,"notAttempted":1/);
    assert.match(content(f.tree), /albumTagEditor.partialCommit/); assert.match(content(f.tree), /albumTagEditor.committedWarning/);
    assert.match(content(f.tree), /albumTagEditor.writeErrors.durability_failed/);
    assert.match(content(f.tree), /albumTagEditor.refreshErrors.mpd_unavailable/);
    for (const track of r.tracks) assert.ok(content(f.tree).includes(track.uri));
  } finally { f.dispose(); }
});

test("catalog/MPD busy and album identity changes during Apply cannot suppress its result", async () => {
  const delayed = deferred(), f = await reviewed({ apply: () => delayed.promise });
  try {
    applyHandler(f)();
    f.props.available = false; f.props.catalogVersion = { mpdBusy: true }; f.props.albumId = "new-id";
    f.render(); f.effects(); f.render();
    assert.equal(f.button("albumTagEditor.cancel").props.disabled, true);
    delayed.resolve(batchResult()); await flush(); f.render();
    assert.match(content(f.tree), /albumTagEditor.resultTitle/); assert.match(content(f.tree), /Music\/a.flac/);
    assert.match(content(f.tree), /albumTagEditor.refresh.accepted/);
    assert.equal(f.calls.finished[0].album_id, "album-id");
    f.props.catalogVersion = {}; f.render(); f.effects(); f.render();
    assert.match(content(f.tree), /albumTagEditor.outcomes.committed/);
    assertClosedStage(f);
  } finally { f.dispose(); }
});

test("results persist through failed Reload and disappear only after an explicitly confirmed new inspection", async () => {
  let inspections = 0;
  const f = await reviewed({ inspect: async () => { if (++inspections === 2) throw new Error("offline"); return physicalInspection(); } });
  try {
    applyHandler(f)(); await flush(); f.render();
    f.button("albumTagEditor.reload").props.onClick(); await flush(); f.render();
    assert.match(content(f.tree), /albumTagEditor.resultTitle/);
    assertClosedStage(f);
    f.props.catalogVersion = {}; f.render(); f.effects(); f.render();
    assert.match(content(f.tree), /albumTagEditor.resultTitle/);
    f.button("albumTagEditor.reload").props.onClick(); await flush(); f.render();
    assert.doesNotMatch(content(f.tree), /albumTagEditor.resultTitle/);
    assertFooter(f, ["albumTagEditor.cancel", "albumTagEditor.preview"]);
  } finally { f.dispose(); }
});

test("unknown errors and invalid responses never claim no changes or retry automatically", async () => {
  for (const service of [{ apply: async () => { throw { code: "album_catalog_failure" }; } }, { apply: async () => ({ ...batchResult(), committed_count: 20 }) }]) {
    const f = await reviewed(service);
    try {
      const click = applyHandler(f); click(); await flush(); f.render(); f.effects(); f.render();
      assert.match(content(f.tree), /albumTagEditor.applyUnknown/);
      assert.doesNotMatch(content(f.tree), /albumTagEditor.previewOnly|albumTagEditor.previewReady/);
      assertClosedStage(f);
      click(); assert.equal(f.calls.apply.length, 1); assert.equal(f.calls.inspect.length, 1);
    } finally { f.dispose(); }
  }
});

test("catalog notification failures cannot turn a reliable committed result into an unknown outcome", async () => {
  const f = await reviewed({}, { onBatchFinished() { throw new Error("subscriber failed"); } });
  try {
    applyHandler(f)(); await flush(); f.render();
    assert.match(content(f.tree), /albumTagEditor.outcomes.committed/);
    assert.match(content(f.tree), /albumTagEditor.catalogNotifyFailed/);
    assert.doesNotMatch(content(f.tree), /albumTagEditor.applyUnknown/);
  } finally { f.dispose(); }
});

test("existing catalog callback waits for accepted jobs and otherwise requests only a catalog refresh", () => {
  const browser = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  const body = browser.match(/onBatchFinished=\{\(result\) => \{([\s\S]*?)\n          \}\}/)?.[1]; assert.ok(body);
  const calls = [], catalog = { waitForMpd() { calls.push("wait"); }, requestRefresh() { calls.push("refresh"); } };
  const notify = new Function("localAlbumCatalog", "result", body);
  notify(catalog, batchResult()); assert.deepEqual(calls.splice(0), ["wait"]);
  const failed = batchResult(); failed.tracks[0].mpd_refresh = { state: "failed", job_id: null, error_code: "mpd_unavailable" };
  notify(catalog, failed); assert.deepEqual(calls.splice(0), ["refresh"]);
  notify(catalog, null); assert.deepEqual(calls.splice(0), ["refresh"]);
  assert.doesNotMatch(body, /rescan|restart|title|artist/);
  const navigation = browser.match(/if \(!playerTarget\) return;([\s\S]*?)\n  \}, \[playerTarget, playerAlbum, tagEditorAlbum\]\)/)?.[1]; assert.ok(navigation);
  let back = 0;
  const navigate = new Function("playerAlbum", "tagEditorAlbum", "onPlayerNavigationBack", "tagEditorTrack", "doctorAlbum", "metadataMode", navigation);
  navigate(null, { id: "old" }, () => { back++; }, null, null, false); assert.equal(back, 0);
  navigate(null, null, () => { back++; }, null, null, false); assert.equal(back, 1);
  navigate(null, null, () => { back++; }, null, null, true); assert.equal(back, 1);
  navigate(null, null, () => { back++; }, null, { id: "old" }, false); assert.equal(back, 1);
});

test("write states and refresh errors are localized and accepted is explicitly pending", () => {
  for (const locale of ["en", "pt-BR"]) {
    const resource = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8")).albumTagEditor;
    for (const state of ["unchanged", "committed", "failed", "not_attempted"]) assert.equal(typeof resource.outcomes[state], "string");
    for (const state of ["not_needed", "accepted", "failed"]) assert.equal(typeof resource.refresh[state], "string");
    assert.match(resource.refresh.accepted, locale === "en" ? /pending.*completion is not confirmed/i : /pendente.*conclusão não confirmada/i);
    assert.match(resource.applyUnknown, locale === "en" ? /may have been changed/ : /podem ter sido alterados/);
    for (const code of ["invalid_local_path", "outside_configured_roots", "file_not_found", "cue_virtual", "unsupported_format", "read_failed", "invalid_tag_update", "revision_conflict", "permission_denied", "transaction_failed", "metadata_preservation_failed", "durability_failed"])
      assert.equal(typeof resource.writeErrors[code], "string");
    for (const code of ["invalid_uri", "mpd_unavailable", "update_command_failed", "invalid_response", "refresh_task_failed"]) assert.equal(typeof resource.refreshErrors[code], "string");
  }
});
