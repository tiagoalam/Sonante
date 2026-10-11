import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { createLibraryDoctorSession, doctorCoverageState, doctorError, doctorFindingGroups, doctorValue,
  installDoctorDialogFocus, libraryDoctorAvailable } from "./libraryDoctor.ts";

const dataModule = code => `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
const invokeMock = dataModule('export const invoke = (command, args) => globalThis.__doctorInvoke(command, args);');
const serviceSource = readFileSync(new URL("../services/libraryDoctor.ts", import.meta.url), "utf8");
const serviceJs = ts.transpileModule(serviceSource, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText
  .replace('from "@tauri-apps/api/core"', `from ${JSON.stringify(invokeMock)}`);
const serviceUrl = dataModule(serviceJs);
const { libraryDoctorService } = await import(serviceUrl);
const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };
const report = (overrides = {}) => ({ album_id: "A", tracks: [{ uri: "Music/a.wav", kind: "physical", issue: null,
  tags: { readable: true, writable: false, editable: false, format: "wav" } }],
  coverage: { canonical_track_count: 1, physical_track_count: 1, physical_tracks_read: 1,
    failed_physical_track_count: 0, cue_virtual_count: 0, physical_inspection_complete: true, partial: false,
    checks_not_performed: ["missing_tracks", "content_duplicates", "canonicalization_discarded_entries"] },
  findings: [], failures: [], ...overrides });
const finding = (code, level = "advisory", evidence = []) => ({ code, level, evidence });
const evidence = (field, value, uri = "Music/a.wav") => ({ uri, field, value });
const catalog = (overrides = {}) => ({ loaded: true, loading: false, error: null, mpdBusy: false,
  saveBlocked: false, albums: [{ id: "A" }], ...overrides });

function fixture() {
  const requests = [], publications = [];
  let available = true;
  globalThis.__doctorInvoke = (command, args) => new Promise((resolve, reject) => requests.push({ command, args, resolve, reject }));
  const session = createLibraryDoctorSession(libraryDoctorService.inspectAlbum, () => available);
  session.subscribe(() => publications.push(session.getSnapshot()));
  const version = {};
  session.syncContext("A", version, true);
  return { session, requests, publications, version, setAvailable(value) { available = value; } };
}

test("successful read uses the actual command and albumId, including readable nonwritable formats", async () => {
  const f = fixture();
  const pending = f.session.inspect();
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].command, "get_local_album_doctor");
  assert.deepEqual(f.requests[0].args, { albumId: "A" });
  f.requests[0].resolve(report()); await pending;
  assert.equal(f.session.getSnapshot().phase, "ready");
  assert.equal(f.session.getSnapshot().report.tracks[0].tags.writable, false);
  f.session.syncContext("A", f.version, true);
  await flush(); assert.equal(f.requests.length, 1);
});

test("every catalog/update/Save gate prevents sending and requires an explicit inspection after recovery", async () => {
  for (const [state, updating] of [[catalog({ loaded: false }), false], [catalog({ loading: true }), false],
    [catalog({ error: "failed" }), false], [catalog({ mpdBusy: true }), false],
    [catalog({ saveBlocked: true }), false], [catalog(), true], [catalog({ albums: [] }), false]]) {
    assert.equal(libraryDoctorAvailable(state, updating, "A"), false);
    const f = fixture(); f.session.syncContext("A", state, libraryDoctorAvailable(state, updating, "A"));
    await f.session.inspect(); assert.equal(f.requests.length, 0);
    f.session.syncContext("A", {}, true); await flush(); assert.equal(f.requests.length, 0);
    const pending = f.session.inspect(); f.requests[0].resolve(report()); await pending;
    assert.equal(f.session.getSnapshot().phase, "ready");
  }
});

test("double sends are blocked synchronously and an explicit new inspection clears the old result", async () => {
  const f = fixture(); const first = f.session.inspect();
  await f.session.inspect(); assert.equal(f.requests.length, 1);
  f.requests[0].resolve(report()); await first;
  const second = f.session.inspect();
  assert.equal(f.session.getSnapshot().report, null);
  assert.equal(f.session.getSnapshot().error, null);
  assert.equal(f.session.getSnapshot().phase, "loading");
  await f.session.inspect(); assert.equal(f.requests.length, 2);
  f.requests[1].resolve(report()); await second;
});

test("identity, catalogVersion and availability invalidate both ready reports and pending replies without auto reload", async () => {
  for (const change of ["identity", "catalog", "availability"]) {
    const f = fixture(); const first = f.session.inspect();
    f.requests[0].resolve(report()); await first;
    const pending = f.session.inspect();
    f.session.syncContext(change === "identity" ? "B" : "A", change === "catalog" ? {} : f.version,
      change !== "availability");
    assert.equal(f.session.getSnapshot().report, null);
    const invalidatedAt = f.publications.length;
    await f.session.inspect(); assert.equal(f.requests.length, 2, "Rust is still reading; no second send");
    f.requests[1].resolve(report()); await pending;
    assert.ok(f.publications.slice(invalidatedAt).every(state => state.report === null));
    assert.notEqual(f.session.getSnapshot().phase, "ready");
    f.session.syncContext("A", {}, true); await flush();
    assert.equal(f.requests.length, 2, "catalog recovery must not reload");
    const explicit = f.session.inspect(); f.requests[2].resolve(report()); await explicit;
    assert.equal(f.session.getSnapshot().phase, "ready");
  }
});

test("an old reply remains invalid after availability returns, even with the original catalog identity", async () => {
  const f = fixture(); const pending = f.session.inspect();
  f.session.syncContext("A", f.version, false); f.session.syncContext("A", f.version, true);
  f.requests[0].resolve(report()); await pending;
  assert.equal(f.session.getSnapshot().report, null);
  assert.equal(f.requests.length, 1);
});

test("live gate changes before the next React render discard replies and errors", async () => {
  for (const rejects of [false, true]) {
    const f = fixture(); const pending = f.session.inspect();
    f.setAvailable(false);
    if (rejects) f.requests[0].reject("task_failed"); else f.requests[0].resolve(report());
    await pending;
    assert.equal(f.session.getSnapshot().phase, "unavailable");
    assert.equal(f.session.getSnapshot().report, null);
    assert.equal(f.session.getSnapshot().error, null);
    f.setAvailable(true); await flush(); assert.equal(f.requests.length, 1);
  }
});

test("close/unmount discards delayed success or rejection and never publishes it", async () => {
  for (const rejects of [false, true]) {
    const f = fixture(); const pending = f.session.inspect();
    f.session.close(); const count = f.publications.length;
    if (rejects) f.requests[0].reject("task_failed"); else f.requests[0].resolve(report());
    await pending; await f.session.inspect();
    assert.equal(f.publications.length, count);
    assert.equal(f.session.getSnapshot().report, null);
    assert.equal(f.requests.length, 1);
    assert.equal(f.session.getSnapshot().phase, "closed");
  }
});

test("reply for a different album is an error, never a displayed report", async () => {
  const f = fixture(); const pending = f.session.inspect();
  f.requests[0].resolve(report({ album_id: "B" })); await pending;
  assert.equal(f.session.getSnapshot().report, null);
  assert.equal(f.session.getSnapshot().error, "report_mismatch");
});

test("DoctorError strings are recognized; unknown strings and code objects use the generic error", async () => {
  for (const cause of ["state_unavailable", "catalog_read_failed", "album_not_found", "task_failed",
    "other", { code: "album_not_found" }, new Error("task_failed")]) {
    const f = fixture(); const pending = f.session.inspect();
    f.requests[0].reject(cause); await pending;
    assert.equal(f.session.getSnapshot().error, doctorError(cause));
    assert.equal(f.session.getSnapshot().report, null);
    assert.equal(f.session.getSnapshot().inFlight, false);
  }
  assert.equal(doctorError({ code: "task_failed" }), "unknown");
});

test("malformed report is an operational error rather than an empty finding list", async () => {
  const f = fixture(); const pending = f.session.inspect();
  f.requests[0].resolve({ album_id: "A" }); await pending;
  assert.equal(f.session.getSnapshot().error, "invalid_report");
  assert.equal(f.session.getSnapshot().report, null);
});

test("null, empty, whitespace and zero evidence values remain distinct", () => {
  assert.deepEqual(doctorValue(null), { kind: "null", text: "null" });
  assert.deepEqual(doctorValue(""), { kind: "empty", text: '""' });
  assert.deepEqual(doctorValue("0"), { kind: "value", text: '"0"' });
  assert.deepEqual(doctorValue(" \t"), { kind: "value", text: '" \\t"' });
});

const translations = {};
for (const language of ["pt-BR", "en"]) {
  const resources = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8"));
  const instance = i18next.createInstance();
  await instance.init({ lng: language, fallbackLng: false, resources: { [language]: { translation: resources } } });
  translations[language] = instance;
}
const i18nMock = dataModule('export const useTranslation = () => ({ t: globalThis.__doctorTranslation.t.bind(globalThis.__doctorTranslation) });');
const componentSource = readFileSync(new URL("../components/LibraryDoctorModal.tsx", import.meta.url), "utf8");
const componentJs = ts.transpileModule(componentSource, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText
  .replace(/from "(react|react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(i18nMock)}`)
  .replace('from "../services/libraryDoctor"', `from ${JSON.stringify(serviceUrl)}`)
  .replace('from "../utils/libraryDoctor"', `from ${JSON.stringify(new URL("./libraryDoctor.ts", import.meta.url).href)}`);
const { LibraryDoctorReportContent, LibraryDoctorModal } = await import(dataModule(componentJs));
const render = (snapshot, language = "pt-BR") => {
  globalThis.__doctorTranslation = translations[language];
  return renderToStaticMarkup(React.createElement(LibraryDoctorReportContent, { report: snapshot }));
};

const partial = () => report({
  coverage: { ...report().coverage, physical_track_count: 2, canonical_track_count: 2,
    failed_physical_track_count: 1, physical_inspection_complete: false, partial: true },
  failures: [{ uri: "Music/failed.flac", error: { code: "read_failed", message: "mensagem sem tradução" } }],
});
const cueOnly = () => report({ tracks: [],
  coverage: { ...report().coverage, physical_track_count: 0, physical_tracks_read: 0,
    canonical_track_count: 2, cue_virtual_count: 2, physical_inspection_complete: true, partial: true,
    checks_not_performed: [...report().coverage.checks_not_performed, "cue_metadata", "cue_numbering"] },
  findings: [finding("cue_checks_not_performed", "advisory", [evidence("cue", "present", "Music/image.cue/track0001")])],
});

test("success and partial IPC reports show scoped findings, read counts and localized per-file failures", async () => {
  for (const snapshot of [report(), partial()]) {
    const f = fixture(); const pending = f.session.inspect(); f.requests[0].resolve(snapshot); await pending;
    for (const language of ["pt-BR", "en"]) {
      const markup = render(f.session.getSnapshot().report, language), t = translations[language].t.bind(translations[language]);
      assert.ok(markup.includes(t("libraryDoctor.noFindings")));
      assert.ok(markup.includes(`1 / ${snapshot.coverage.physical_track_count}`));
      assert.ok(markup.includes(t(`libraryDoctor.coverageStates.${snapshot.coverage.partial ? "partial" : "performed"}`)));
      if (snapshot.failures.length) {
        assert.ok(markup.includes("Music/failed.flac"));
        assert.ok(markup.includes(t("libraryDoctor.fileErrors.read_failed")));
        assert.doesNotMatch(markup, /mensagem sem tradução/);
      }
    }
  }
});

test("CUE-only and zero-read IPC reports cannot look like a complete or healthy diagnosis", async () => {
  for (const snapshot of [cueOnly(), report({ coverage: { ...report().coverage, physical_tracks_read: 0 } }),
    report({ coverage: { ...report().coverage, physical_tracks_read: 0, physical_track_count: 0, canonical_track_count: 0 } }),
    report({ ...partial(), coverage: { ...partial().coverage, physical_tracks_read: 0 } })]) {
    const f = fixture(); const pending = f.session.inspect(); f.requests[0].resolve(snapshot); await pending;
    assert.equal(doctorCoverageState(f.session.getSnapshot().report), "no_reads");
    for (const language of ["pt-BR", "en"]) {
      const markup = render(snapshot, language), t = translations[language].t.bind(translations[language]);
      assert.ok(markup.includes(t("libraryDoctor.coverageStates.no_reads")));
      assert.ok(!markup.includes(t("libraryDoctor.coverageStates.performed")));
      assert.ok(!markup.includes(t("libraryDoctor.noFindings")));
      if (!snapshot.findings.length) assert.ok(markup.includes(t("libraryDoctor.noReadableConclusion")));
      if (snapshot.coverage.cue_virtual_count) {
        assert.ok(markup.includes(t("libraryDoctor.checks.cue_metadata")));
        assert.ok(markup.includes(t("libraryDoctor.checks.cue_numbering")));
      }
    }
  }
});

test("finding groups distinguish all levels and render expandable evidence with original values", () => {
  const snapshot = report({ findings: [
    finding("album_artist_absent", "advisory", [evidence("album_artist", null)]),
    finding("album_artist_absent", "advisory", [evidence("album_artist", "")]),
    finding("track_exceeds_total", "inconsistency", [evidence("track_number", "0")]),
    finding("possible_number_conflict", "possible_conflict", [evidence("date", " 2000-02-29 ")]),
  ] });
  assert.deepEqual(doctorFindingGroups(snapshot).map(group => group.findings.length), [2, 1, 1]);
  for (const language of ["pt-BR", "en"]) {
    const markup = render(snapshot, language), t = translations[language].t.bind(translations[language]);
    assert.equal((markup.match(/<details\b/g) ?? []).length, 4);
    assert.equal((markup.match(/<h4\b/g) ?? []).length, 3);
    for (const level of ["advisory", "inconsistency", "possible_conflict"]) assert.ok(markup.includes(t(`libraryDoctor.levels.${level}`)));
    assert.ok(markup.includes(t("libraryDoctor.valueNull")));
    assert.ok(markup.includes(t("libraryDoctor.valueEmpty").replaceAll('"', '&quot;')));
    assert.ok(markup.includes('&quot;0&quot;'));
    assert.ok(markup.includes('&quot; 2000-02-29 &quot;'));
    assert.ok(markup.includes("Music/a.wav"));
  }
});

test("all backend codes, errors, fields, checks and limits have independent pt-BR/en translations", () => {
  const rust = readFileSync(new URL("../../src-tauri/src/library_doctor.rs", import.meta.url), "utf8");
  const mapping = { DoctorError: "errors", FindingCode: "codes", FindingLevel: "levels", Field: "fields", UnperformedCheck: "checks" };
  const snake = value => value.replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase();
  for (const [enumeration, group] of Object.entries(mapping)) {
    const values = rust.match(new RegExp(`pub enum ${enumeration} \\{([^}]+)\\}`))[1].split(",").map(value => value.trim()).filter(Boolean);
    for (const value of values) for (const language of ["pt-BR", "en"]) {
      const key = `libraryDoctor.${group}.${snake(value)}`;
      assert.ok(translations[language].exists(key), `${language}: ${key}`);
      assert.notEqual(translations[language].t(key), key);
    }
  }
  const flatten = (value, prefix = "") => Object.entries(value).flatMap(([key, item]) => typeof item === "string"
    ? [[`${prefix}${key}`, item]] : flatten(item, `${prefix}${key}.`));
  const pt = flatten(translations["pt-BR"].getResource("pt-BR", "translation", "libraryDoctor"));
  const en = new Map(flatten(translations.en.getResource("en", "translation", "libraryDoctor")));
  assert.deepEqual(pt.map(([key]) => key).sort(), [...en.keys()].sort());
  for (const [key, message] of pt) {
    assert.ok(message.trim()); assert.ok(en.get(key).trim());
    assert.deepEqual(message.match(/{{\w+}}/g), en.get(key).match(/{{\w+}}/g));
  }
});

test("modal markup keeps closing and explicit inspection, scrolling, labels and interpretation limits", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__doctorTranslation = translations[language];
    const markup = renderToStaticMarkup(React.createElement(LibraryDoctorModal, {
      albumId: "A", displayName: "Album", available: false, catalogVersion: {}, isAvailable: () => false, onClose() {},
    }));
    assert.match(markup, /role="dialog".*aria-modal="true".*aria-labelledby=.*aria-describedby=/);
    assert.ok(markup.includes("overflow-y-auto")); assert.ok(markup.includes("max-h-[92vh]"));
    assert.match(markup, /<button[^>]*disabled=""/);
    for (const limit of ["membership", "dates", "numbers", "conflicts", "cue", "snapshot", "invalidation"]) {
      assert.ok(markup.includes(translations[language].t(`libraryDoctor.limits.${limit}`)));
    }
    assert.equal((markup.match(/<button\b/g) ?? []).length, 3, "only close, inspect again, and close");
  }
});

test("focus enters and stays in the dialog, Escape closes, and cleanup restores focus and scroll", () => {
  const listeners = new Map(), doc = { body: { style: { overflow: "auto" } }, activeElement: null,
    addEventListener(name, fn) { listeners.set(name, fn); }, removeEventListener(name, fn) { assert.equal(listeners.get(name), fn); listeners.delete(name); } };
  const element = () => ({ isConnected: true, focus() { doc.activeElement = this; }, getClientRects() { return [1]; } });
  const previous = element(), first = element(), last = element(), hidden = { ...element(), getClientRects() { return []; } };
  const root = { ...element(), ownerDocument: doc, querySelectorAll: () => [first, hidden, last],
    contains: item => [root, first, last].includes(item) };
  doc.activeElement = previous;
  let closed = 0, prevented = 0;
  const cleanup = installDoctorDialogFocus(root, () => closed++);
  const key = (value, shiftKey = false) => listeners.get("keydown")({ key: value, shiftKey,
    preventDefault() { prevented++; }, stopPropagation() {} });
  assert.equal(doc.activeElement, root); assert.equal(doc.body.style.overflow, "hidden");
  key("Tab"); assert.equal(doc.activeElement, first);
  key("Tab", true); assert.equal(doc.activeElement, last);
  key("Tab"); assert.equal(doc.activeElement, first);
  listeners.get("focusin")({ target: previous }); assert.equal(doc.activeElement, root);
  key("Escape"); assert.equal(closed, 1); assert.equal(prevented, 4);
  cleanup(); assert.equal(doc.activeElement, previous); assert.equal(doc.body.style.overflow, "auto");
  assert.equal(listeners.size, 0);
});

// Replay the actual mount effects with a minimal hook host, following the
// existing transpiled-component test pattern without starting the desktop app.
const hookMock = dataModule(`
  import React from ${JSON.stringify(import.meta.resolve("react"))};
  export default React;
  export const useRef = value => ({ current: value });
  export const useMemo = factory => factory();
  export const useCallback = callback => callback;
  export const useId = () => "doctor-test";
  export const useSyncExternalStore = (_, snapshot) => snapshot();
  export const useEffect = setup => globalThis.__doctorMountEffects.push(setup);
`);
const { LibraryDoctorModal: MountModal } = await import(dataModule(componentJs.replace(
  `from ${JSON.stringify(import.meta.resolve("react"))}`, `from ${JSON.stringify(hookMock)}`,
)));

test("opening dispatches once despite StrictMode effect replay, and cleanup invalidates the pending read", async () => {
  const f = fixture(); globalThis.__doctorMountEffects = [];
  globalThis.__doctorTranslation = translations.en;
  MountModal({ albumId: "A", displayName: "Album", available: true, catalogVersion: {},
    isAvailable: () => true, onClose() {} });
  const setups = globalThis.__doctorMountEffects;
  for (const cleanup of setups.map(setup => setup())) cleanup?.();
  const cleanups = setups.map(setup => setup());
  await flush();
  assert.equal(f.requests.length, 1);
  assert.deepEqual(f.requests[0].args, { albumId: "A" });
  for (const cleanup of cleanups) cleanup?.();
  f.requests[0].resolve(report()); await flush();
  assert.equal(f.requests.length, 1);
});

test("closing before the deferred opening inspection sends no IPC", async () => {
  const f = fixture(); globalThis.__doctorMountEffects = [];
  globalThis.__doctorTranslation = translations.en;
  const element = MountModal({ albumId: "A", displayName: "Album", available: true, catalogVersion: {},
    isAvailable: () => true, onClose() {} });
  const cleanups = globalThis.__doctorMountEffects.map(setup => setup());
  element.props.onClick({ target: element, currentTarget: element });
  for (const cleanup of cleanups) cleanup?.();
  await flush(); assert.equal(f.requests.length, 0);
});

test("closing and reopening cannot overlap Rust reads or reuse the old snapshot", async () => {
  const old = fixture(); const pending = old.session.inspect();
  old.session.close();
  const reopened = fixture(); await reopened.session.inspect();
  assert.equal(reopened.requests.length, 0);
  assert.equal(reopened.session.getSnapshot().error, "read_in_progress");
  assert.equal(reopened.session.getSnapshot().report, null);
  old.requests[0].resolve(report()); await pending; await flush();
  assert.equal(reopened.requests.length, 0, "finishing an old read must not start a new one");
  assert.equal(reopened.session.getSnapshot().report, null);
  const explicit = reopened.session.inspect();
  assert.equal(reopened.requests.length, 1);
  reopened.requests[0].resolve(report()); await explicit;
  assert.equal(reopened.session.getSnapshot().phase, "ready");
});
