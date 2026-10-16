import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { doctorReviewField, doctorReviewRequest } from "./libraryDoctorReview.ts";
import { analyzeLocalAlbumTagForm } from "./localAlbumTagEditor.ts";

const version = {};
const tags = (overrides = {}) => ({ title: null, artist: null, album: "Fresh album", album_artist: "Fresh artist",
  genre: null, date: "2001-02", track_number: 1, track_total: 1, disc_number: null, disc_total: null,
  format: "flac", readable: true, writable: true, editable: true, unsupported_reason: null, revision: "fresh-revision", ...overrides });
const report = () => ({ album_id: "A", tracks: [{ uri: "Music/a.flac", kind: "physical", issue: null,
  tags: tags({ album_artist: null, date: "DOCTOR date", revision: "doctor-revision" }) }],
  coverage: { canonical_track_count: 1, physical_track_count: 1, physical_tracks_read: 1,
    failed_physical_track_count: 0, cue_virtual_count: 0, physical_inspection_complete: true, partial: false,
    checks_not_performed: ["missing_tracks", "content_duplicates", "canonicalization_discarded_entries"] },
  findings: ["album_artist_absent", "album_artist_empty", "year_divergence", "date_uninterpretable"].map(code =>
    ({ code, level: "advisory", evidence: [] })), failures: [] });
const state = () => ({ albumId: "A", catalogVersion: version, available: true, phase: "ready", report: report(), error: null, inFlight: false });
const current = () => ({ albumId: "A", catalogVersion: version, available: true, editorOpen: false });
const inspection = (overrides = {}) => ({ album_id: "A", tracks: [{ uri: "Music/a.flac", kind: "physical", tags: tags(), issue: null }],
  fields: { album: { state: "common", value: "Fresh album" }, album_artist: { state: "common", value: "Fresh artist" },
    date: { state: "mixed", value: null }, genre: { state: "absent", value: null } },
  physical_track_count: 1, cue_virtual_count: 0, inspection_complete: true, ...overrides });

test("only the approved codes map to fields, with no tags, revisions, values or write request in the handoff", () => {
  for (const [code, field] of [["album_artist_absent", "album_artist"], ["album_artist_empty", "album_artist"], ["year_divergence", "date"]]) {
    assert.equal(doctorReviewField(code), field);
    assert.deepEqual(doctorReviewRequest(state(), code, current()), { albumId: "A", code, field });
  }
  for (const code of ["date_uninterpretable", "number_indeterminate", "track_exceeds_total", "disc_exceeds_total", "possible_number_conflict", "cue_checks_not_performed"]) {
    assert.equal(doctorReviewField(code), null);
    assert.equal(doctorReviewRequest(state(), code, current()), null);
  }
});

test("stale, pending, unavailable, mismatched and absent findings cannot navigate", () => {
  for (const change of [s => s.phase = "stale", s => s.inFlight = true, s => s.available = false,
    s => s.albumId = "B", s => s.report.album_id = "B", s => s.catalogVersion = {},
    s => s.report = null, s => s.report.findings = []]) {
    const s = state(); change(s); assert.equal(doctorReviewRequest(s, "year_divergence", current()), null);
  }
  for (const override of [{ albumId: "B" }, { catalogVersion: {} }, { available: false }, { editorOpen: true }]) {
    assert.equal(doctorReviewRequest(state(), "album_artist_empty", { ...current(), ...override }), null);
  }
});

test("complete physical coverage and editable formats are required, independently of diagnostic readability", () => {
  for (const change of [r => r.coverage.partial = true, r => r.coverage.physical_inspection_complete = false,
    r => r.coverage.physical_tracks_read = 0, r => r.coverage.physical_track_count = 0,
    r => r.coverage.cue_virtual_count = 1, r => r.coverage.failed_physical_track_count = 1,
    r => r.coverage.canonical_track_count = 2, r => r.failures.push({ uri: "Music/a.flac", error: { code: "read_failed" } }),
    r => r.tracks = [], r => r.tracks[0].kind = "cue_virtual", r => r.tracks[0].tags = null,
    r => r.tracks[0].issue = { code: "read_failed" }, r => r.tracks[0].tags.readable = false,
    r => r.tracks[0].tags.writable = false, r => r.tracks[0].tags.editable = false]) {
    const s = state(); change(s.report); assert.equal(doctorReviewRequest(s, "year_divergence", current()), null);
  }
});

const dataModule = code => `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
const source = relative => readFileSync(new URL(relative, import.meta.url), "utf8");
const invokeUrl = dataModule('export const invoke = (command, args) => globalThis.__reviewInvoke(command, args);');
const transpile = text => ts.transpileModule(text, { compilerOptions: {
  jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020,
} }).outputText;
const serviceUrl = relative => dataModule(transpile(source(relative)).replace('from "@tauri-apps/api/core"', `from ${JSON.stringify(invokeUrl)}`));
const hooksUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))}; export default React;
  ${["useState", "useRef", "useEffect", "useMemo", "useCallback", "useId", "useSyncExternalStore"].map(name =>
    `export const ${name} = (...args) => globalThis.__reviewHost.${name}(...args);`).join("\n")}`);
const translationUrl = dataModule('export const useTranslation = () => ({t: (...args) => globalThis.__reviewTranslation.t(...args)});');
const translations = {};
for (const language of ["pt-BR", "en"]) {
  const instance = i18next.createInstance();
  await instance.init({ lng: language, resources: { [language]: { translation: JSON.parse(source(`../locales/${language}.json`)) } }, interpolation: { escapeValue: false } });
  translations[language] = instance;
}
function componentModule(relative, mocks) {
  return dataModule(transpile(source(relative)).replace(/from "([^"]+)"/g, (full, name) => {
    const url = name === "react" ? hooksUrl : name === "react-i18next" ? translationUrl
      : mocks[name] ?? (name.startsWith(".") ? new URL(`${name}.ts`, new URL(relative, import.meta.url)).href : import.meta.resolve(name));
    return `from ${JSON.stringify(url)}`;
  }));
}
const { LocalAlbumTagEditorModal: Editor } = await import(componentModule("../components/LocalAlbumTagEditorModal.tsx", {
  "../services/localTags": serviceUrl("../services/localTags.ts"),
  "./OnlineAlbumMetadataPanel": dataModule('export const OnlineAlbumMetadataPanel = () => null;'),
}));
const { LibraryDoctorModal: Doctor, LibraryDoctorReportContent: Content } = await import(componentModule("../components/LibraryDoctorModal.tsx", {
  "../services/libraryDoctor": serviceUrl("../services/libraryDoctor.ts"),
}));
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function host(Component, props) {
  const slots = [], effects = [];
  let cursor = 0;
  const api = {
    useState(initial) {
      const i = cursor++;
      if (!(i in slots)) slots[i] = typeof initial === "function" ? initial() : initial;
      return [slots[i], value => slots[i] = typeof value === "function" ? value(slots[i]) : value];
    },
    useRef(initial) { const i = cursor++; return slots[i] ??= { current: initial }; },
    useMemo(factory, deps) {
      const i = cursor++;
      if (!slots[i] || deps.some((d, j) => d !== slots[i].deps[j])) slots[i] = { deps, value: factory() };
      return slots[i].value;
    },
    useCallback(fn, deps) { return api.useMemo(() => fn, deps); },
    useId() { return "review-test"; },
    useSyncExternalStore(_, snapshot) { return snapshot(); },
    useEffect(setup, deps) {
      const i = cursor++, previous = slots[i];
      if (!previous || !deps || deps.some((d, j) => d !== previous.deps[j])) {
        effects.push(() => { previous?.cleanup?.(); slots[i] = { deps, cleanup: setup() }; });
      }
    },
    render(next = props) { props = next; globalThis.__reviewHost = api; cursor = 0; return Component(props); },
    effects() { effects.splice(0).forEach(setup => setup()); },
    cleanup() { slots.forEach(slot => slot?.cleanup?.()); },
    slots,
  };
  return api;
}
function nodes(tree, predicate) {
  const found = [];
  function walk(node) {
    if (Array.isArray(node)) return node.forEach(walk);
    if (!node || typeof node !== "object") return;
    if (predicate(node)) found.push(node);
    walk(node.props?.children);
  }
  walk(tree); return found;
}
const props = extra => ({ albumId: "A", displayName: "Album", catalogVersion: version, available: true, isAvailable: () => true, onClose() {}, ...extra });
globalThis.HTMLElement = class {};
globalThis.document = { activeElement: null, addEventListener() {}, removeEventListener() {} };
globalThis.window = { confirm() { throw new Error("Unexpected confirmation"); } };

test("explicit Doctor click rechecks the live gate and closes its session only on accepted navigation", async () => {
  globalThis.__reviewTranslation = translations.en;
  let available = true, accept = false;
  const calls = [], contexts = [];
  globalThis.__reviewInvoke = async (command, args) => { calls.push({ command, args }); return report(); };
  const h = host(Doctor, props({ isAvailable: () => available, onReview: context => { contexts.push(context); return accept; } }));
  h.render(); h.effects(); await flush();
  let content = nodes(h.render(), n => n.type === Content)[0];
  assert.equal(content.props.canReview("album_artist_absent"), true);
  available = false; content.props.onReview("album_artist_absent"); assert.equal(contexts.length, 0);
  available = true; content.props.onReview("album_artist_absent"); assert.equal(contexts.length, 1);
  assert.equal(content.props.canReview("album_artist_absent"), true, "rejected handoff keeps the report");
  accept = true; content.props.onReview("year_divergence");
  assert.deepEqual(contexts[1], { albumId: "A", code: "year_divergence", field: "date" });
  assert.equal(nodes(h.render(), n => n.type === Content).length, 0, "Doctor cannot overlay the editor");
  await flush(); assert.deepEqual(calls, [{ command: "get_local_album_doctor", args: { albumId: "A" } }]);
  h.cleanup();
});

test("saved callbacks cannot navigate after catalog or album invalidation", async () => {
  globalThis.__reviewTranslation = translations.en;
  for (const override of [{ albumId: "B" }, { catalogVersion: {} }, { available: false }]) {
    let navigated = 0;
    globalThis.__reviewInvoke = async () => report();
    const p = props({ onReview() { navigated++; return true; } }), h = host(Doctor, p);
    h.render(); h.effects(); await flush();
    const old = nodes(h.render(), n => n.type === Content)[0];
    h.render({ ...p, ...override }); old.props.onReview("year_divergence");
    assert.equal(navigated, 0); h.cleanup();
  }
});

test("blocked and foreign-album IPC reports never offer an enabled review", async () => {
  globalThis.__reviewTranslation = translations.en;
  for (const change of [r => r.coverage.partial = true, r => r.coverage.cue_virtual_count = 1,
    r => r.coverage.physical_tracks_read = 0, r => r.tracks[0].tags.editable = false,
    r => r.failures.push({ uri: "Music/a.flac", error: { code: "read_failed" } }), r => r.album_id = "B"]) {
    const snapshot = report(); change(snapshot);
    let navigated = 0;
    globalThis.__reviewInvoke = async () => snapshot;
    const h = host(Doctor, props({ onReview() { navigated++; return true; } }));
    h.render(); h.effects(); await flush();
    const content = nodes(h.render(), n => n.type === Content)[0];
    if (snapshot.album_id === "B") assert.equal(content, undefined);
    else { assert.equal(content.props.canReview("year_divergence"), false); content.props.onReview("year_divergence"); }
    assert.equal(navigated, 0); h.cleanup();
  }
});

test("editor always inspects afresh, focuses the hint, and starts unchanged without preview or Apply", async () => {
  globalThis.__reviewTranslation = translations.en;
  for (const code of ["album_artist_absent", "album_artist_empty", "year_divergence"]) {
    const calls = [], context = doctorReviewRequest(state(), code, current());
    let resolve;
    globalThis.__reviewInvoke = (command, args) => { calls.push({ command, args }); return new Promise(r => resolve = r); };
    const h = host(Editor, props({ reviewContext: context }));
    assert.equal(nodes(h.render(), n => n.type === "input").length, 0);
    h.effects(); assert.deepEqual(calls, [{ command: "get_local_album_tag_inspection", args: { albumId: "A" } }]);
    resolve(inspection()); await flush();
    const tree = h.render(), inputs = nodes(tree, n => n.type === "input");
    assert.equal(inputs.find(n => n.props.id === "local-album-tag-album_artist").props.value, "Fresh artist");
    assert.equal(inputs.find(n => n.props.id === "local-album-tag-date").props.value, "");
    assert.deepEqual(inputs.filter(n => n.props.autoFocus).map(n => n.props.id), [`local-album-tag-${context.field}`]);
    assert.equal(inputs.find(n => n.props.autoFocus).props["aria-describedby"], "local-album-tag-doctor-context");
    const draft = h.slots.find(s => s?.inspection?.album_id === "A");
    assert.ok(Object.values(draft.fields).every(field => !field.touched && !field.clearAll));
    assert.deepEqual(analyzeLocalAlbumTagForm(draft).update, Object.fromEntries(["album", "album_artist", "date", "genre"].map(field => [field, { action: "unchanged" }])));
    assert.equal(draft.inspection.tracks[0].tags.revision, "fresh-revision");
    assert.equal(nodes(tree, n => n.key === "apply").length, 0);
    assert.equal(nodes(tree, n => n.key === "preview")[0].props.disabled, true);
    assert.equal(nodes(tree, n => n.props?.role === "note").length, 1);
    await flush(); assert.equal(calls.length, 1); h.cleanup();
  }
});

test("fresh editor inspection remains the authority when the format becomes blocked", async () => {
  globalThis.__reviewTranslation = translations.en;
  const blocked = inspection({ tracks: [{ uri: "Music/a.wav", kind: "physical", issue: null, tags: tags({ writable: false, editable: false }) }] });
  globalThis.__reviewInvoke = async () => blocked;
  const h = host(Editor, props({ reviewContext: doctorReviewRequest(state(), "year_divergence", current()) }));
  h.render(); h.effects(); await flush();
  const inputs = nodes(h.render(), n => n.type === "input");
  assert.ok(inputs.length && inputs.every(n => n.props.disabled));
  assert.equal(analyzeLocalAlbumTagForm(h.slots.find(s => s?.inspection)).blockedReason, "format");
  h.cleanup();
});

test("ordinary TAG-2 opening and wrong-album hints have the original focus and no advisory banner", async () => {
  globalThis.__reviewTranslation = translations.en;
  for (const reviewContext of [undefined, { albumId: "B", code: "year_divergence", field: "date" }]) {
    const calls = [];
    globalThis.__reviewInvoke = async (command, args) => { calls.push({ command, args }); return inspection(); };
    const h = host(Editor, props({ reviewContext })); h.render(); h.effects(); await flush();
    const tree = h.render();
    assert.equal(nodes(tree, n => n.props?.role === "note").length, 0);
    assert.deepEqual(nodes(tree, n => n.type === "input" && n.props.autoFocus).map(n => n.props.id), ["local-album-tag-album"]);
    assert.equal(calls.length, 1); h.cleanup();
  }
});

test("review labels, gate explanation and advisory context are localized and disabled on blocked coverage", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__reviewTranslation = translations[language];
    const markup = renderToStaticMarkup(React.createElement(Content, { report: report(), canReview: () => false, onReview() {} }));
    const t = translations[language].t.bind(translations[language]);
    assert.equal((markup.match(/<button\b/g) ?? []).length, 3);
    assert.equal((markup.match(/disabled=""/g) ?? []).length, 3);
    for (const key of ["albumArtist", "dates", "requirements", "context"]) {
      assert.ok(translations[language].exists(`libraryDoctor.review.${key}`));
      if (key !== "context") assert.ok(markup.includes(t(`libraryDoctor.review.${key}`)));
    }
    const h = host(Editor, props({ reviewContext: { albumId: "A", code: "year_divergence", field: "date" } }));
    const note = nodes(h.render(), n => n.props?.role === "note")[0];
    assert.equal(note.props.children, t("libraryDoctor.review.context", { finding: t("libraryDoctor.codes.year_divergence") }));
    assert.ok(note.props.children.includes(language === "en" ? "fresh inspection" : "inspeção fresca"));
  }
});

// Execute the actual view callback, extracted via the TypeScript AST, with
// synthetic catalog/editor state rather than rendering the full browser/app.
const viewSource = source("../components/LocalBrowserView.tsx");
const viewAst = ts.createSourceFile("view.tsx", viewSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function jsxAttribute(name, owner, title) {
  let expression;
  function visit(node) {
    if ((ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && node.tagName.getText(viewAst) === owner) {
      if (title && !node.attributes.properties.some(p => p.name?.getText(viewAst) === "title"
        && p.initializer?.expression?.getText(viewAst).includes(title))) return;
      const attr = node.attributes.properties.find(p => p.name?.getText(viewAst) === name);
      expression = attr?.initializer?.expression;
    }
    ts.forEachChild(node, visit);
  }
  visit(viewAst); assert.ok(expression, `${owner}.${name}`); return expression.getText(viewAst);
}
test("view handoff revalidates identity/catalog/availability, preserves active drafts and sends no automatic I/O", () => {
  const callback = jsxAttribute("onReview", "LibraryDoctorModal");
  for (const fail of [null, "draft", "identity", "catalog", "availability", "album"]) {
    const changes = [], open = { current: fail === "draft" }, album = { id: "A", title: "Album" };
    const handler = new Function("tagEditorOpen", "doctorAlbum", "doctorSelectedAlbum", "localAlbumCatalog", "catalog", "albumTagEditingAvailable", "setDoctorAlbum", "setDoctorReviewContext", "setTagEditorAlbum", `return (${callback});`)(
      open, album, { current: fail === "identity" ? "B" : "A" }, { getSnapshot: () => fail === "catalog" ? {} : version }, version,
      () => fail !== "availability", value => changes.push(["doctor", value]), value => changes.push(["context", value]), value => changes.push(["editor", value]));
    const context = { albumId: fail === "album" ? "B" : "A", code: "year_divergence", field: "date" };
    assert.equal(handler(context), !fail);
    assert.deepEqual(changes, fail ? [] : [["doctor", null], ["context", context], ["editor", album]]);
    if (!fail) { assert.equal(handler(context), false, "double navigation cannot replace an active draft"); assert.equal(changes.length, 3); }
  }
  const closed = [];
  new Function("setTagEditorAlbum", "setDoctorReviewContext", `return (${jsxAttribute("onClose", "LocalAlbumTagEditorModal")});`)(
    value => closed.push(["editor", value]), value => closed.push(["context", value]))();
  assert.deepEqual(closed, [["editor", null], ["context", null]], "close returns to the album without reopening or re-inspecting Doctor");
});

test("album review entry preserves the exact identity and cannot replace an active dialog", () => {
  for (const active of [false, true]) {
    const changes = [], open = { current: active }, album = { id: "A" };
    const click = new Function("tagEditorOpen", "doctorAlbum", "selectedAlbum", "setMetadataAlbumId", "onSubviewChange",
      `return (${jsxAttribute("onClick", "button", "localMetadata.review")});`)(open, null, album,
      value => changes.push(["selection", value]), value => changes.push(["subview", value]));
    click();
    assert.deepEqual(changes, active ? [] : [["selection", "A"], ["subview", "metadata"]]);
    assert.equal(open.current, active, "navigation must not start an editor or inspection");
  }
});
