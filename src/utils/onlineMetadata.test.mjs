import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { createOnlineMetadataSession, onlineMetadataSeed } from "./onlineMetadata.ts";

const source = path => readFileSync(new URL(path, import.meta.url), "utf8");
const dataModule = code => `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
const compile = code => ts.transpileModule(code, { compilerOptions: {
  jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020,
} }).outputText;
const invokeUrl = dataModule('export const invoke = (command, args) => globalThis.__metadataInvoke(command, args);');
const serviceUrl = dataModule(compile(source("../services/onlineMetadata.ts")).replace('from "@tauri-apps/api/core"', `from ${JSON.stringify(invokeUrl)}`));
const { onlineMetadataService } = await import(serviceUrl);
const inspection = () => ({ album_id: "A", tracks: [], fields: { album: { state: "common", value: "Fresh album" },
  album_artist: { state: "absent", value: null }, date: { state: "mixed", value: null }, genre: { state: "absent", value: null } },
  physical_track_count: 1, cue_virtual_count: 0, inspection_complete: true });
const result = () => ({ total: 21, limited: true, candidates: [{ id: "62d1c4ef-fc00-37af-8df7-485f6a31fcc4", title: "Edition",
  artist_credit: "A feat. B & C", date: "2001-03", country: "BR", formats: ["CD", null],
  labels: [{ name: "Label", catalog_number: "XYZ" }, { name: null, catalog_number: "" }], score: 0 }] });
const flush = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
function fixture() {
  const requests = [], context = { albumId: "A", catalogVersion: {}, inspection: inspection(), available: true };
  let available = true;
  globalThis.__metadataInvoke = (command, args) => new Promise((resolve, reject) => requests.push({ command, args, resolve, reject }));
  const session = createOnlineMetadataSession(onlineMetadataService.search, () => available);
  session.syncContext(context);
  return { session, context, requests, live(value) { available = value; } };
}

test("seed uses only common fresh album/Album Artist fields, never track artist or an inferred artist", () => {
  const i = inspection(); i.tracks = [{ tags: { artist: "Do not infer" } }];
  assert.deepEqual(onlineMetadataSeed(i), { title: "Fresh album", artist: "" });
  for (const state of ["absent", "mixed", "indeterminate", "not_applicable"]) {
    i.fields.album = i.fields.album_artist = { state, value: "Do not use" };
    assert.deepEqual(onlineMetadataSeed(i), { title: "", artist: "" });
  }
  i.fields.album_artist = { state: "common", value: "A & B" };
  assert.equal(onlineMetadataSeed(i).artist, "A & B");
});

test("no search on mount/context/query changes; explicit search sends separate query once and blocks duplicate dispatch", async () => {
  const f = fixture(); await flush(); assert.equal(f.requests.length, 0);
  f.session.setQuery("artist", "User artist"); await flush(); assert.equal(f.requests.length, 0);
  const pending = f.session.search(); await f.session.search();
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].command, "search_online_album_metadata");
  assert.deepEqual(f.requests[0].args, { request: { title: "Fresh album", artist: "User artist" } });
  f.requests[0].resolve(result()); await pending;
  assert.equal(f.session.getSnapshot().phase, "ready");
  assert.ok(!("selected" in f.session.getSnapshot()));
  const again = f.session.search(); assert.equal(f.session.getSnapshot().result, null);
  f.requests[1].resolve({ candidates: [], total: 0, limited: false }); await again;
  assert.deepEqual(f.session.getSnapshot().result.candidates, []);
});

test("query/album/catalog/inspection/availability changes discard delayed successes and failures, with no automatic retry", async () => {
  for (const change of ["query", "album", "catalog", "inspection", "availability", "live", "close"]) {
    for (const failure of [false, true]) {
      const f = fixture(), pending = f.session.search();
      if (change === "query") f.session.setQuery("title", "Changed");
      else if (change === "close") f.session.close();
      else if (change === "live") f.live(false);
      else f.session.syncContext({ ...f.context, ...({ album: { albumId: "B" }, catalog: { catalogVersion: {} },
        inspection: { inspection: inspection() }, availability: { available: false } }[change]) });
      await f.session.search(); assert.equal(f.requests.length, 1);
      if (failure) f.requests[0].reject("timeout"); else f.requests[0].resolve(result());
      await pending; await flush();
      assert.equal(f.session.getSnapshot().result, null); assert.equal(f.session.getSnapshot().error, null);
      assert.equal(f.session.getSnapshot().inFlight, false); assert.equal(f.requests.length, 1);
    }
  }
});

test("ready reports disappear immediately on context or query changes; old generations cannot become current again", async () => {
  const f = fixture(); const pending = f.session.search(); f.requests[0].resolve(result()); await pending;
  f.session.setQuery("artist", "Different"); assert.equal(f.session.getSnapshot().result, null);
  const next = f.session.search(); f.session.setQuery("artist", "Again"); f.session.setQuery("artist", "Different");
  f.requests[1].resolve(result()); await next; assert.equal(f.session.getSnapshot().result, null);
  f.session.syncContext({ ...f.context, inspection: { ...inspection(), album_id: "B" } });
  assert.deepEqual(f.session.getSnapshot().query, { title: "", artist: "" });
});

test("stable errors, invalid queries/results and unavailable guards never look like empty success", async () => {
  for (const error of ["service_unavailable", "rate_limited", "timeout", "http_error", "invalid_response", "response_too_large", { code: "timeout" }]) {
    const f = fixture(), pending = f.session.search(); f.requests[0].reject(error); await pending;
    assert.equal(f.session.getSnapshot().result, null);
    assert.equal(f.session.getSnapshot().error, typeof error === "string" ? error : "unknown");
  }
  for (const title of ["", "A\nB", "x".repeat(257)]) {
    const f = fixture(); f.session.setQuery("title", title); await f.session.search();
    assert.equal(f.requests.length, 0); assert.equal(f.session.getSnapshot().error, "invalid_query");
  }
  for (const invalid of [null, {}, { candidates: [], total: -1, limited: false }, { ...result(), candidates: Array(11).fill(result().candidates[0]) }]) {
    const f = fixture(), pending = f.session.search(); f.requests[0].resolve(invalid); await pending;
    assert.equal(f.session.getSnapshot().error, "invalid_response");
  }
  const f = fixture(); f.live(false); await f.session.search(); assert.equal(f.requests.length, 0);
});

const translations = {};
for (const language of ["pt-BR", "en"]) {
  const instance = i18next.createInstance();
  await instance.init({ lng: language, resources: { [language]: { translation: JSON.parse(source(`../locales/${language}.json`)) } }, interpolation: { escapeValue: false } });
  translations[language] = instance;
}
const translationUrl = dataModule('export const useTranslation = () => ({ t: (...args) => globalThis.__metadataTranslation.t(...args) });');
const comparisonJs = compile(source("../components/OnlineMetadataComparison.tsx"))
  .replace(/from "(react|react\/jsx-runtime)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/onlineMetadataTransfer"', `from ${JSON.stringify(new URL("./onlineMetadataTransfer.ts", import.meta.url).href)}`);
const panelJs = compile(source("../components/OnlineAlbumMetadataPanel.tsx"))
  .replace(/from "(react|react\/jsx-runtime)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "./OnlineMetadataComparison"', `from ${JSON.stringify(dataModule(comparisonJs))}`)
  .replace('from "../services/onlineMetadata"', `from ${JSON.stringify(serviceUrl)}`)
  .replace('from "../utils/onlineMetadata"', `from ${JSON.stringify(new URL("./onlineMetadata.ts", import.meta.url).href)}`);
const { OnlineMetadataResults, OnlineAlbumMetadataPanel } = await import(dataModule(panelJs));
test("candidate display preserves credits, edition date, missing/empty/zero, limited and empty results in both languages", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__metadataTranslation = translations[language];
    const t = translations[language].t.bind(translations[language]);
    const markup = renderToStaticMarkup(React.createElement(OnlineMetadataResults, { result: result() }));
    assert.ok(markup.includes("A feat. B &amp; C") && markup.includes("2001-03") && markup.includes("XYZ"));
    assert.ok(markup.includes(t("onlineMetadata.missing")) && markup.includes(t("onlineMetadata.empty")));
    assert.ok(markup.includes(">0</dd>") && !markup.includes("0%"));
    assert.ok(markup.includes(t("onlineMetadata.limited", { count: 1, total: 21 })));
    assert.doesNotMatch(markup, /<button|<input|aria-selected|<img/);
    const empty = renderToStaticMarkup(React.createElement(OnlineMetadataResults, { result: { candidates: [], total: 0, limited: false } }));
    assert.ok(empty.includes(t("onlineMetadata.noResults")));
    const panel = renderToStaticMarkup(React.createElement(OnlineAlbumMetadataPanel, {
      albumId: "A", catalogVersion: {}, inspection: inspection(), available: true, isAvailable: () => true,
    }));
    assert.ok(panel.includes(t("onlineMetadata.limits")));
    assert.match(panel, /value="Fresh album"/); assert.match(panel, /value=""/);
    assert.doesNotMatch(panel, /<form|type="submit"|<img/);
    const keys = Object.keys(translations[language].getResource(language, "translation", "onlineMetadata.errors"));
    assert.equal(keys.length, 11); keys.forEach(key => assert.ok(translations[language].exists(`onlineMetadata.errors.${key}`)));
  }
});

// A hook host runs the real editor without starting the app or a DOM renderer.
const hooksUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))}; export default React;
  ${["useState", "useRef", "useEffect", "useMemo"].map(name => `export const ${name} = (...args) => globalThis.__metadataHost.${name}(...args);`).join("\n")}`);
const tagServiceUrl = dataModule(compile(source("../services/localTags.ts")).replace('from "@tauri-apps/api/core"', `from ${JSON.stringify(invokeUrl)}`));
const editorJs = compile(source("../components/LocalAlbumTagEditorModal.tsx"))
  .replace('from "react"', `from ${JSON.stringify(hooksUrl)}`)
  .replace(/from "(react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/onlineMetadataTransfer"', `from ${JSON.stringify(new URL("./onlineMetadataTransfer.ts", import.meta.url).href)}`)
  .replace('from "../services/localTags"', `from ${JSON.stringify(tagServiceUrl)}`)
  .replace('from "../utils/localAlbumTagEditor"', `from ${JSON.stringify(new URL("./localAlbumTagEditor.ts", import.meta.url).href)}`)
  .replace('from "./OnlineAlbumMetadataPanel"', `from ${JSON.stringify(dataModule(panelJs))}`);
const { LocalAlbumTagEditorModal: Editor } = await import(dataModule(editorJs));
function host(Component, props) {
  let cursor = 0;
  const slots = [], effects = [];
  const h = {
    useState(initial) { const i = cursor++; if (!(i in slots)) slots[i] = initial; return [slots[i], value => slots[i] = typeof value === "function" ? value(slots[i]) : value]; },
    useRef(initial) { return slots[cursor++] ??= { current: initial }; },
    useMemo(factory, deps) { const i = cursor++; if (!slots[i] || deps.some((value, j) => value !== slots[i].deps[j])) slots[i] = { value: factory(), deps }; return slots[i].value; },
    useEffect(setup, deps) { const i = cursor++, previous = slots[i]; if (!previous || deps.some((value, j) => value !== previous.deps[j])) effects.push(() => { previous?.cleanup?.(); slots[i] = { deps, cleanup: setup() }; }); },
    render(next = props) { props = next; globalThis.__metadataHost = h; cursor = 0; return Component(props); },
    effects() { effects.splice(0).forEach(setup => setup()); },
    close() { slots.forEach(slot => slot?.cleanup?.()); }, slots,
  };
  return h;
}
function find(tree, predicate) {
  if (Array.isArray(tree)) return tree.flatMap(node => find(node, predicate));
  if (!tree || typeof tree !== "object") return [];
  return [...(predicate(tree) ? [tree] : []), ...find(tree.props?.children, predicate)];
}
test("real editor integration uses fresh inspection, preserves dirty draft and preview, and sends no tag writes while searching", async () => {
  globalThis.__metadataTranslation = translations.en;
  globalThis.HTMLElement = class {};
  globalThis.document = { activeElement: null, addEventListener() {}, removeEventListener() {} };
  globalThis.window = { confirm() { throw new Error("Unexpected confirmation"); } };
  const fresh = inspection();
  fresh.tracks = [{ uri: "Music/a.flac", kind: "physical", issue: null, tags: { revision: "fresh", readable: true, writable: true, editable: true } }];
  const calls = [];
  globalThis.__metadataInvoke = async (command, args) => { calls.push({ command, args }); return command === "get_local_album_tag_inspection" ? fresh : result(); };
  const p = { albumId: "A", displayName: "Do not seed", catalogVersion: {}, available: true, isAvailable: () => true, onClose() {},
    reviewContext: { albumId: "A", field: "album_artist", code: "album_artist_absent" } };
  const h = host(Editor, p); h.render(); h.effects(); await flush();
  let tree = h.render();
  find(tree, node => node.type === "input" && node.props.id === "local-album-tag-album")[0].props.onChange({ target: { value: "User draft" } });
  tree = h.render();
  const draft = h.slots.find(slot => slot?.inspection === fresh), before = JSON.stringify(draft);
  const panel = find(tree, node => node.type === OnlineAlbumMetadataPanel)[0];
  assert.equal(panel.props.inspection, fresh); assert.equal(panel.props.catalogVersion, p.catalogVersion);
  assert.equal(panel.props.available, true);
  const session = createOnlineMetadataSession(onlineMetadataService.search, panel.props.isAvailable);
  session.syncContext(panel.props); assert.equal(session.getSnapshot().query.title, "Fresh album");
  session.setQuery("artist", "Query only"); await session.search();
  assert.equal(JSON.stringify(h.slots.find(slot => slot?.inspection === fresh)), before);
  assert.deepEqual(calls.map(call => call.command), ["get_local_album_tag_inspection", "search_online_album_metadata"]);
  // A pre-existing preview stays the same object; query state never touches it.
  const previewIndex = 4, preview = { view: { canApply: false, files: [] }, request: {}, token: {} };
  h.slots[previewIndex] = preview;
  session.setQuery("title", "Another query"); await session.search();
  assert.equal(h.slots[previewIndex], preview);
  h.render({ ...p, catalogVersion: {} });
  assert.equal(find(h.render(), node => node.type === OnlineAlbumMetadataPanel)[0].props.available, false);
  session.close(); h.close();
});

test("query Enter is isolated from TAG-2 form submission and the shared backend limiter/UA exposure is minimal", () => {
  const panelSource = source("../components/OnlineAlbumMetadataPanel.tsx");
  const ast = ts.createSourceFile("panel.tsx", panelSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let handler;
  const visit = node => { if (ts.isJsxAttribute(node) && node.name.getText(ast) === "onKeyDown") handler = node.initializer.expression.getText(ast); ts.forEachChild(node, visit); };
  visit(ast); assert.ok(handler);
  globalThis.HTMLInputElement = class {};
  let prevented = 0, stopped = 0;
  new Function(`return (${handler});`)()({ key: "Enter", target: new HTMLInputElement(), preventDefault() { prevented++; }, stopPropagation() { stopped++; } });
  assert.equal(prevented, 1); assert.equal(stopped, 1);
  const artwork = source("../../src-tauri/src/online_artwork.rs"), backend = source("../../src-tauri/src/online_metadata.rs");
  assert.match(artwork, /pub\(crate\) fn wait_for_musicbrainz/); assert.match(backend, /spawn_blocking\(crate::online_artwork::wait_for_musicbrainz\)/);
  assert.match(backend, /user_agent\(crate::online_artwork::USER_AGENT\)/);
  assert.doesNotMatch(backend, /LAST_MUSICBRAINZ_REQUEST|coverartarchive/);
});
