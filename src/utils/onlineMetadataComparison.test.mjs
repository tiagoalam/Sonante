import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { createOnlineMetadataSession } from "./onlineMetadata.ts";
import { applyOnlineMetadataValues, validMetadataDate, transferReason } from "./onlineMetadataTransfer.ts";
import { createLocalAlbumTagEditorDraft, editLocalAlbumTagField } from "./localAlbumTagEditor.ts";

const ID = "62d1c4ef-fc00-37af-8df7-485f6a31fcc4", OTHER = "12345678-abcd-1234-abcd-123456789abc";
const inspection = () => ({ album_id: "A", physical_track_count: 1, cue_virtual_count: 0, inspection_complete: true,
  fields: { album: { state: "common", value: "Inspected" }, album_artist: { state: "absent", value: null },
    date: { state: "mixed", value: null }, genre: { state: "common", value: "Jazz" } },
  tracks: [{ uri: "synthetic/a.flac", kind: "physical", issue: null, tags: { readable: true, writable: true, editable: true, revision: "fresh" } }] });
const details = () => ({ id: ID, title: "Edition", artist_credit: "A feat. B & C", edition_date: "2004-03-01", release_group_id: OTHER, first_release_date: "1999" });
const result = () => ({ candidates: [ID, OTHER].map(id => ({ id, title: "Edition", artist_credit: "A", date: "2004", country: null, formats: [], labels: [], score: 0 })), total: 2, limited: false });
const source = path => readFileSync(new URL(path, import.meta.url), "utf8");
const dataModule = code => `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
const compile = text => ts.transpileModule(text, { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
const invokeUrl = dataModule('export const invoke = (command, args) => globalThis.__comparisonInvoke(command, args);');
const serviceUrl = dataModule(compile(source("../services/onlineMetadata.ts")).replace('from "@tauri-apps/api/core"', `from ${JSON.stringify(invokeUrl)}`));
const { onlineMetadataService } = await import(serviceUrl);
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
async function fixture() {
  const requests = [], i = inspection();
  let live = true;
  const context = { albumId: "A", catalogVersion: {}, inspection: i, available: true, draft: createLocalAlbumTagEditorDraft(i), editorRevision: "1:2", transferAvailable: true };
  globalThis.__comparisonInvoke = (command, args) => new Promise((resolve, reject) => requests.push({ command, args, resolve, reject }));
  const session = createOnlineMetadataSession(onlineMetadataService.search, () => live, onlineMetadataService.details);
  session.syncContext(context);
  const search = session.search(); requests[0].resolve(result()); await search;
  return { session, context, requests, live(value) { live = value; }, async compare(value = details()) {
    const p = session.compare(ID); requests.at(-1).resolve(value); await p;
  } };
}

test("comparison fetches only after explicit candidate click, blocks duplicate sends and leaves the draft untouched", async () => {
  const f = await fixture(), draft = f.context.draft, before = structuredClone(draft);
  assert.equal(f.requests.length, 1); assert.equal(f.session.getSnapshot().details, null);
  const pending = f.session.compare(ID); await f.session.compare(OTHER); await f.session.search();
  assert.equal(f.requests.length, 2);
  assert.deepEqual(f.requests[1].args, { mbid: ID });
  assert.equal(f.requests[1].command, "get_online_album_metadata_details");
  f.requests[1].resolve(details()); await pending;
  assert.deepEqual(f.session.getSnapshot().choices, []); assert.equal(f.session.getSnapshot().dateSource, null);
  assert.equal(f.session.transfer(() => { throw Error("no selection"); }), false);
  assert.deepEqual(draft, before);
});

test("explicit fields and date source transfer with TAG-2 semantics while preserving other manual values", async () => {
  const f = await fixture();
  f.context.draft = editLocalAlbumTagField(editLocalAlbumTagField(f.context.draft, "genre", "Manual genre"), "album", "Manual album");
  f.session.syncContext(f.context); await f.compare();
  f.session.choose("date", true); assert.deepEqual(f.session.getSnapshot().choices, []);
  f.session.chooseDateSource("first_release"); assert.deepEqual(f.session.getSnapshot().choices, []);
  f.session.choose("album_artist", true); f.session.choose("date", true);
  f.session.chooseDateSource("edition");
  assert.deepEqual(f.session.getSnapshot().choices, ["album_artist"], "changing origin requires selecting Date again");
  f.session.choose("date", true);
  let editionValues;
  assert.equal(f.session.transfer(r => { editionValues = r.values; return true; }), true);
  assert.deepEqual(editionValues, { album_artist: "A feat. B & C", date: "2004-03-01" });
  f.session.chooseDateSource("first_release"); f.session.choose("album_artist", true); f.session.choose("date", true);
  let next, request;
  assert.equal(f.session.transfer(r => { request = r; assert.equal(r.isCurrent(), true); next = applyOnlineMetadataValues(r.draft, r.values); return !!next; }), true);
  assert.deepEqual(request.values, { album_artist: "A feat. B & C", date: "1999" });
  assert.equal(next.fields.album, f.context.draft.fields.album); assert.equal(next.fields.genre, f.context.draft.fields.genre);
  assert.equal(next.fields.date.value, "1999"); assert.equal(next.fields.date.touched, true); assert.equal(next.fields.date.clearAll, false);
  assert.equal(f.context.draft.fields.album_artist.touched, false);
  assert.equal(f.session.transfer(() => { throw Error("duplicate transfer"); }), false);
});

test("dates require exact valid calendar precision; absent and empty values remain unavailable without inference", async () => {
  for (const good of ["0001", "1999", "2024-02", "2024-02-29", "2000-02-29", "2023-12-31"]) assert.equal(validMetadataDate(good), true, good);
  for (const bad of ["0000", "2023-02-29", "1900-02-29", "2024-00", "2024-13", "2024-04-31", "2024-01-00", "2024-1", "2024-01-01T00:00:00Z", " 2024", "2024 ", "unknown", "10000"]) assert.equal(validMetadataDate(bad), false, bad);
  assert.equal(transferReason(null), "missing"); assert.equal(transferReason(""), "empty"); assert.equal(transferReason("  "), "empty");
  const f = await fixture(); await f.compare({ ...details(), title: "", artist_credit: null, edition_date: "2023-02-29", first_release_date: null });
  for (const field of ["album", "album_artist", "date"]) f.session.choose(field, true);
  f.session.chooseDateSource("edition"); f.session.chooseDateSource("first_release");
  assert.deepEqual(f.session.getSnapshot().choices, []); assert.equal(f.session.getSnapshot().dateSource, null);
  assert.equal(applyOnlineMetadataValues(f.context.draft, { date: "2023-02-29" }), null);
});

test("draft changes clear field/date choices while retaining comparison against the current draft", async () => {
  const f = await fixture(); await f.compare(); f.session.choose("album", true); f.session.chooseDateSource("edition"); f.session.choose("date", true);
  const loaded = f.session.getSnapshot().details;
  f.session.syncContext({ ...f.context, draft: editLocalAlbumTagField(f.context.draft, "album_artist", "Manual"), editorRevision: "1:3" });
  assert.equal(f.session.getSnapshot().details, loaded); assert.deepEqual(f.session.getSnapshot().choices, []); assert.equal(f.session.getSnapshot().dateSource, null);
  assert.equal(f.session.transfer(() => { throw Error("old choices"); }), false);
});

test("query, identity, catalog, fresh inspection, availability, revision, draft and close discard late details and errors", async () => {
  for (const change of ["query", "album", "catalog", "inspection", "available", "revision", "draft", "live", "close"]) {
    for (const failure of [false, true]) {
      const f = await fixture(), pending = f.session.compare(ID);
      if (change === "query") f.session.setQuery("artist", "Changed");
      else if (change === "close") f.session.close();
      else if (change === "live") f.live(false);
      else f.session.syncContext({ ...f.context, ...({ album: { albumId: "B" }, catalog: { catalogVersion: {} },
        inspection: { inspection: inspection() }, available: { available: false }, revision: { editorRevision: "1:3" },
        draft: { draft: editLocalAlbumTagField(f.context.draft, "genre", "New") } }[change]) });
      if (failure) f.requests[1].reject("timeout"); else f.requests[1].resolve(details()); await pending;
      assert.equal(f.session.getSnapshot().details, null, change); assert.equal(f.session.getSnapshot().detailError, null, change);
      assert.deepEqual(f.session.getSnapshot().choices, []); assert.equal(f.requests.length, 2);
    }
  }
});

test("switching candidate clears details and selections; malformed identity and lookup failures never become successful comparison", async () => {
  const f = await fixture(); await f.compare(); f.session.choose("album", true);
  const pending = f.session.compare(OTHER); assert.equal(f.session.getSnapshot().details, null); assert.deepEqual(f.session.getSnapshot().choices, []);
  f.requests.at(-1).resolve(details()); await pending; assert.equal(f.session.getSnapshot().detailError, "identity_mismatch");
  for (const cause of ["release_not_found", "invalid_mbid", "rate_limited", "timeout", "service_unavailable", "invalid_response", "response_too_large"]) {
    const p = f.session.compare(ID); f.requests.at(-1).reject(cause); await p;
    assert.equal(f.session.getSnapshot().details, null); assert.equal(f.session.getSnapshot().detailError, cause);
  }
  for (const invalid of [{ ...details(), id: "bad" }, { ...details(), artist_credit: undefined }, { ...details(), release_group_id: null }, { ...details(), release_group_id: "bad" }]) {
    await f.compare(invalid); assert.equal(f.session.getSnapshot().detailError, "invalid_response");
  }
});

test("CUE, partial and uneditable inspections cannot transfer; availability and current details are revalidated on click", async () => {
  for (const mutate of [i => i.cue_virtual_count = 1, i => i.inspection_complete = false,
    i => i.physical_track_count = 0, i => i.tracks[0].tags.editable = false, i => i.tracks[0].tags.writable = false,
    i => i.tracks[0].tags.readable = false, i => i.tracks[0].issue = "failed"]) {
    const draft = createLocalAlbumTagEditorDraft(inspection()); mutate(draft.inspection);
    assert.equal(applyOnlineMetadataValues(draft, { album: "Edition" }), null);
  }
  for (const change of ["live", "context", "details", "transfer"]) {
    const f = await fixture(); await f.compare(); f.session.choose("album", true);
    if (change === "live") f.live(false);
    if (change === "context") f.session.syncContext({ ...f.context, catalogVersion: {} });
    if (change === "details") { const p = f.session.compare(OTHER); f.requests.at(-1).resolve({ ...details(), id: OTHER }); await p; }
    if (change === "transfer") f.session.syncContext({ ...f.context, transferAvailable: false });
    assert.equal(f.session.transfer(() => { throw Error("must block"); }), false);
  }
});

const translations = {};
for (const language of ["pt-BR", "en"]) {
  const instance = i18next.createInstance();
  await instance.init({ lng: language, resources: { [language]: { translation: JSON.parse(source(`../locales/${language}.json`)) } }, interpolation: { escapeValue: false } });
  translations[language] = instance;
}
const translationUrl = dataModule('export const useTranslation = () => ({ t: (...args) => globalThis.__comparisonTranslation.t(...args) });');
const transferUrl = new URL("./onlineMetadataTransfer.ts", import.meta.url).href;
const comparisonUrl = dataModule(compile(source("../components/OnlineMetadataComparison.tsx"))
  .replace(/from "(react|react\/jsx-runtime)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/onlineMetadataTransfer"', `from ${JSON.stringify(transferUrl)}`));
const { OnlineMetadataComparison } = await import(comparisonUrl);
const panelUrl = dataModule(compile(source("../components/OnlineAlbumMetadataPanel.tsx"))
  .replace(/from "(react|react\/jsx-runtime)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "./OnlineMetadataComparison"', `from ${JSON.stringify(comparisonUrl)}`)
  .replace('from "../services/onlineMetadata"', `from ${JSON.stringify(serviceUrl)}`)
  .replace('from "../utils/onlineMetadata"', `from ${JSON.stringify(new URL("./onlineMetadata.ts", import.meta.url).href)}`));
const { OnlineAlbumMetadataPanel, OnlineMetadataResults } = await import(panelUrl);

test("comparison in both languages distinguishes inspected absent/empty/mixed, manual draft, distinct date sources and unavailable reasons", () => {
  for (const language of ["pt-BR", "en"]) {
    globalThis.__comparisonTranslation = translations[language]; const t = translations[language].t.bind(translations[language]);
    const draft = createLocalAlbumTagEditorDraft(inspection()); draft.inspection.fields.album = { state: "common", value: "" };
    draft.fields.album_artist = { ...draft.fields.album_artist, touched: true, value: "Manual credit" };
    const markup = renderToStaticMarkup(React.createElement(OnlineMetadataComparison, { details: details(), draft, available: true,
      choices: [], dateSource: null, onChoose() {}, onDateSource() {}, onTransfer() {} }));
    for (const text of ["2004-03-01", "1999", "Manual credit", t("onlineMetadata.empty"), t("albumTagEditor.states.absent"), t("albumTagEditor.states.mixed"), t("onlineMetadata.comparison.reasons.chooseDateSource")]) assert.ok(markup.includes(text), text);
    assert.ok(markup.includes("A feat. B &amp; C")); assert.doesNotMatch(markup, /checked=""|type="submit"/);
    const unavailable = renderToStaticMarkup(React.createElement(OnlineMetadataComparison, { details: { ...details(), title: null, artist_credit: "", edition_date: "2023-02-29" }, draft, available: false,
      choices: [], dateSource: null, onChoose() {}, onDateSource() {}, onTransfer() {} }));
    for (const reason of ["missing", "empty", "invalidDate"]) assert.ok(unavailable.includes(t(`onlineMetadata.comparison.reasons.${reason}`)));
    assert.ok(unavailable.includes(t("onlineMetadata.comparison.blocked")));
    const candidates = renderToStaticMarkup(React.createElement(OnlineMetadataResults, { result: result(), onCompare() {} }));
    assert.equal((candidates.match(/<button/g) || []).length, 2); assert.ok(candidates.indexOf(ID) < candidates.indexOf(OTHER));
    for (const key of ["invalid_mbid", "release_not_found", "identity_mismatch"]) assert.ok(translations[language].exists(`onlineMetadata.errors.${key}`));
  }
});

// Same hook-host pattern as the existing frontend tests, running the real editor with mocked IPC.
const hooksUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))}; export default React;
 ${["useState", "useRef", "useEffect", "useMemo"].map(name => `export const ${name} = (...args) => globalThis.__comparisonHost.${name}(...args);`).join("\n")}`);
const tagServiceUrl = dataModule(compile(source("../services/localTags.ts")).replace('from "@tauri-apps/api/core"', `from ${JSON.stringify(invokeUrl)}`));
const editorUrl = dataModule(compile(source("../components/LocalAlbumTagEditorModal.tsx"))
  .replace('from "react"', `from ${JSON.stringify(hooksUrl)}`)
  .replace(/from "(react\/jsx-runtime|lucide-react)"/g, (_, name) => `from ${JSON.stringify(import.meta.resolve(name))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "../utils/onlineMetadataTransfer"', `from ${JSON.stringify(transferUrl)}`)
  .replace('from "../services/localTags"', `from ${JSON.stringify(tagServiceUrl)}`)
  .replace('from "../utils/localAlbumTagEditor"', `from ${JSON.stringify(new URL("./localAlbumTagEditor.ts", import.meta.url).href)}`)
  .replace('from "./OnlineAlbumMetadataPanel"', `from ${JSON.stringify(panelUrl)}`));
const { LocalAlbumTagEditorModal: Editor } = await import(editorUrl);
function host(Component, props) {
  let cursor = 0; const slots = [], effects = [];
  const h = {
    useState(initial) { const i = cursor++; if (!(i in slots)) slots[i] = initial; return [slots[i], value => slots[i] = typeof value === "function" ? value(slots[i]) : value]; },
    useRef(initial) { return slots[cursor++] ??= { current: initial }; },
    useMemo(factory, deps) { const i = cursor++; if (!slots[i] || deps.some((value, j) => value !== slots[i].deps[j])) slots[i] = { value: factory(), deps }; return slots[i].value; },
    useEffect(setup, deps) { const i = cursor++, previous = slots[i]; if (!previous || deps.some((value, j) => value !== previous.deps[j])) effects.push(() => { previous?.cleanup?.(); slots[i] = { deps, cleanup: setup() }; }); },
    render(next = props) { props = next; globalThis.__comparisonHost = h; cursor = 0; return Component(props); },
    effects() { effects.splice(0).forEach(setup => setup()); }, close() { slots.forEach(slot => slot?.cleanup?.()); }, slots,
  }; return h;
}
function find(tree, predicate) {
  if (Array.isArray(tree)) return tree.flatMap(node => find(node, predicate));
  if (!tree || typeof tree !== "object") return [];
  return [...(predicate(tree) ? [tree] : []), ...find(tree.props?.children, predicate)];
}
async function editorFixture() {
  globalThis.__comparisonTranslation = translations.en;
  globalThis.HTMLElement = class {};
  globalThis.document = { activeElement: null, addEventListener() {}, removeEventListener() {} };
  globalThis.window = { confirm() { return true; } };
  const calls = [], fresh = inspection();
  globalThis.__comparisonInvoke = async (command, args) => { calls.push({ command, args });
    if (command === "get_local_album_tag_inspection") return fresh;
    if (command === "search_online_album_metadata") return result();
    if (command === "get_online_album_metadata_details") return details();
    throw Error(`unexpected command ${command}`);
  };
  const p = { albumId: "A", displayName: "Album", catalogVersion: {}, available: true, isAvailable: () => true, onClose() {}, reviewContext: { albumId: "A", field: "album_artist", code: "album_artist_absent" } };
  const h = host(Editor, p); h.render(); h.effects(); await flush();
  const panel = () => find(h.render(), node => node.type === OnlineAlbumMetadataPanel)[0].props;
  return { h, p, fresh, calls, panel };
}

test("real editor comparison preserves draft and preview until explicit transfer; selected transfer invalidates preview with no preflight/Apply", async () => {
  const f = await editorFixture();
  find(f.h.render(), n => n.type === "input" && n.props.id === "local-album-tag-genre")[0].props.onChange({ target: { value: "Manual genre" } });
  f.h.render(); const guard = f.h.slots.find(slot => slot?.current?.version && slot.current.capture).current;
  const preview = { view: { canApply: false, files: [] }, request: {}, token: guard.capture() }; f.h.slots[4] = preview;
  const props = f.panel(), before = props.draft;
  const session = createOnlineMetadataSession(onlineMetadataService.search, props.isAvailable, onlineMetadataService.details);
  session.syncContext(props); await session.search(); await session.compare(ID);
  assert.equal(f.h.slots[0], before); assert.equal(f.h.slots[4], preview);
  assert.equal(before.fields.album_artist.touched, false); session.choose("album_artist", true);
  assert.equal(session.transfer(props.onTransfer), true);
  assert.equal(f.h.slots[4], null); assert.equal(guard.isCurrent(preview.token), false);
  assert.equal(f.h.slots[0].fields.album_artist.value, "A feat. B & C"); assert.equal(f.h.slots[0].fields.album_artist.touched, true);
  assert.equal(f.h.slots[0].fields.genre, before.fields.genre); assert.equal(f.h.slots[0].fields.album, before.fields.album);
  assert.deepEqual(f.calls.map(c => c.command), ["get_local_album_tag_inspection", "search_online_album_metadata", "get_online_album_metadata_details"]);
  session.close(); f.h.close();
});

test("editor rejects a transfer from stale render, reload, preflight, consumed revision, unavailable catalog or forged inspection", async () => {
  for (const block of ["edit", "reload", "previewing", "consumed", "catalog", "identity", "inspection", "close", "apply", "preflight"]) {
    const f = await editorFixture(), props = f.panel();
    const session = createOnlineMetadataSession(onlineMetadataService.search, props.isAvailable, onlineMetadataService.details);
    session.syncContext(props); await session.search(); await session.compare(ID); session.choose("album", true);
    if (block === "edit") find(f.h.render(), n => n.type === "input" && n.props.id === "local-album-tag-genre")[0].props.onChange({ target: { value: "Manual" } });
    if (block === "reload") { find(f.h.render(), n => n.type === "button" && React.Children.toArray(n.props.children).includes(translations.en.t("albumTagEditor.reload")))[0].props.onClick(); }
    if (block === "previewing") { f.h.slots[3] = true; session.syncContext(f.panel()); }
    if (block === "apply") f.h.slots[8].current = true;
    if (block === "consumed") f.h.slots[9].current = true;
    if (block === "catalog") f.h.render({ ...f.p, catalogVersion: {} });
    if (block === "identity") f.h.render({ ...f.p, albumId: "B" });
    if (block === "close") find(f.h.render(), n => n.type === "button" && n.props["aria-label"] === translations.en.t("albumTagEditor.close"))[0].props.onClick();
    if (block === "preflight") f.h.slots.find(slot => slot?.current?.version && slot.current.capture).current.capture();
    const before = f.h.slots[0];
    assert.equal(session.transfer(request => props.onTransfer(block === "inspection" ? { ...request, inspection: inspection() } : request)), false, block);
    assert.equal(f.h.slots[0], before, block); session.close(); f.h.close(); await flush();
  }
});

// Run the actual panel and its candidate button handlers using the existing hook host.
// The host models store subscriptions/rerenders; it is not a DOM/React scheduler or Tauri runtime.
const panelHooksUrl = dataModule(`import React from ${JSON.stringify(import.meta.resolve("react"))}; export default React;
 ${["useRef", "useEffect", "useMemo", "useId", "useSyncExternalStore"].map(name => `export const ${name} = (...args) => globalThis.__comparisonHost.${name}(...args);`).join("\n")}`);
const interactivePanelUrl = dataModule(compile(source("../components/OnlineAlbumMetadataPanel.tsx"))
  .replace('from "react"', `from ${JSON.stringify(panelHooksUrl)}`)
  .replace('from "react/jsx-runtime"', `from ${JSON.stringify(import.meta.resolve("react/jsx-runtime"))}`)
  .replace('from "react-i18next"', `from ${JSON.stringify(translationUrl)}`)
  .replace('from "./OnlineMetadataComparison"', `from ${JSON.stringify(comparisonUrl)}`)
  .replace('from "../services/onlineMetadata"', `from ${JSON.stringify(serviceUrl)}`)
  .replace('from "../utils/onlineMetadata"', `from ${JSON.stringify(new URL("./onlineMetadata.ts", import.meta.url).href)}`));
const { OnlineAlbumMetadataPanel: InteractivePanel, OnlineMetadataResults: InteractiveResults } = await import(interactivePanelUrl);
function panelInteraction(language = "en") {
  globalThis.__comparisonTranslation = translations[language];
  const requests = [], inspected = inspection(), draft = editLocalAlbumTagField(createLocalAlbumTagEditorDraft(inspected), "genre", "Manual genre");
  const originalDraft = structuredClone(draft);
  let transferCalls = 0, tree, storeRenders = 0;
  const props = { albumId: "A", catalogVersion: {}, inspection: inspected, available: true, draft,
    editorRevision: "1:2", transferAvailable: true, isAvailable: () => true, onTransfer() { transferCalls++; return false; } };
  globalThis.__comparisonInvoke = (command, args) => new Promise((resolve, reject) => requests.push({ command, args, resolve, reject }));
  const h = host(InteractivePanel, props), render = h.render;
  h.render = next => tree = render(next);
  h.useId = () => h.useRef("online-panel-interaction").current;
  h.useSyncExternalStore = (subscribe, getSnapshot) => {
    h.useEffect(() => subscribe(() => { storeRenders++; h.render(); }), [subscribe]);
    return getSnapshot();
  };
  h.render(); h.effects();
  const t = translations[language].t.bind(translations[language]);
  const candidates = () => find(tree, node => node.type === InteractiveResults)[0];
  return {
    props, requests, h, t, candidates,
    markup: () => renderToStaticMarkup(tree),
    comparison: () => find(tree, node => node.type === OnlineMetadataComparison)[0],
    clickSearch() {
      const button = find(tree, node => node.type === "button" && node.props.children === t("onlineMetadata.search"))[0];
      assert.ok(button && !button.props.disabled); button.props.onClick();
    },
    clickCompare(index = 0) {
      // Expand the real candidate component to find its real button, keeping its onCompare callback from the panel.
      const buttons = find(InteractiveResults(candidates().props), node => node.type === "button");
      assert.ok(buttons[index] && !buttons[index].props.disabled); buttons[index].props.onClick();
    },
    changeArtist(value) {
      find(tree, node => node.type === "input" && node.props.id.endsWith("-artist"))[0].props.onChange({ target: { value } });
    },
    assertDraftPreserved() { assert.deepEqual(draft, originalDraft); assert.equal(transferCalls, 0); },
    storeRenders: () => storeRenders,
  };
}
function assertBeforeCandidateList(markup, content) {
  const at = markup.indexOf(content), list = markup.indexOf("<ol");
  assert.ok(at >= 0 && list >= 0 && at < list, `Expected ${content} before candidate list`);
}
async function populatePanel(f) {
  assert.equal(f.requests.length, 0, "mount must not search");
  f.clickSearch();
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].command, "search_online_album_metadata");
  assert.deepEqual(f.requests[0].args, { request: { title: "Inspected", artist: "" } });
  const candidates = result(); f.requests[0].resolve(candidates); await flush();
  assert.equal(f.candidates().props.result, candidates);
}

test("real panel button → simulated IPC → subscribed rerender shows loading and comparison before unchanged candidate list", async () => {
  for (const language of ["pt-BR", "en"]) {
    const f = panelInteraction(language);
    try {
      await populatePanel(f);
      const listBefore = renderToStaticMarkup(React.createElement(InteractiveResults, f.candidates().props));
      f.clickCompare(1);
      assert.equal(f.requests.length, 2);
      assert.equal(f.requests[1].command, "get_online_album_metadata_details");
      assert.deepEqual(f.requests[1].args, { mbid: OTHER });
      assertBeforeCandidateList(f.markup(), f.t("onlineMetadata.comparison.loading"));
      assert.equal(f.comparison(), undefined);
      f.requests[1].resolve({ ...details(), id: OTHER }); await flush();
      assert.ok(f.comparison()); assert.equal(f.comparison().props.details.id, OTHER);
      assertBeforeCandidateList(f.markup(), f.t("onlineMetadata.comparison.title"));
      assert.ok(!f.markup().includes(f.t("onlineMetadata.comparison.loading")));
      assert.deepEqual(f.comparison().props.choices, []); assert.equal(f.comparison().props.dateSource, null);
      assert.equal(renderToStaticMarkup(React.createElement(InteractiveResults, f.candidates().props)), listBefore);
      assert.ok(f.storeRenders() >= 5, "store publications must rerender the actual panel");
      f.assertDraftPreserved();
    } finally { f.h.close(); }
  }
});

test("real panel button → rejected IPC → subscribed rerender shows lookup error before candidates without comparison", async () => {
  for (const language of ["pt-BR", "en"]) {
    const f = panelInteraction(language);
    try {
      await populatePanel(f); f.clickCompare();
      assertBeforeCandidateList(f.markup(), f.t("onlineMetadata.comparison.loading"));
      f.requests[1].reject("timeout"); await flush();
      assertBeforeCandidateList(f.markup(), f.t("onlineMetadata.errors.timeout"));
      assert.match(f.markup(), /role="alert"/); assert.equal(f.comparison(), undefined);
      assert.ok(!f.markup().includes(f.t("onlineMetadata.comparison.loading")));
      assert.equal(f.candidates().props.disabled, false); f.assertDraftPreserved();
    } finally { f.h.close(); }
  }
});

test("real panel drops delayed lookup success/error after query or catalog invalidation without reopening comparison on new search", async () => {
  for (const invalidate of ["query", "catalog"]) {
    for (const failed of [false, true]) {
      const f = panelInteraction();
      try {
        await populatePanel(f); f.clickCompare();
        assertBeforeCandidateList(f.markup(), f.t("onlineMetadata.comparison.loading"));
        if (invalidate === "query") f.changeArtist("Different query");
        else f.h.render({ ...f.props, catalogVersion: {} });
        assert.equal(f.candidates(), undefined); assert.equal(f.comparison(), undefined);
        assert.ok(!f.markup().includes(f.t("onlineMetadata.comparison.loading")));
        if (failed) f.requests[1].reject("timeout"); else f.requests[1].resolve(details());
        await flush();
        assert.equal(f.comparison(), undefined); assert.doesNotMatch(f.markup(), /role="alert"/);
        f.clickSearch(); assert.equal(f.requests.length, 3);
        assert.equal(f.requests[2].command, "search_online_album_metadata");
        f.requests[2].resolve(result()); await flush();
        assert.ok(f.candidates()); assert.equal(f.comparison(), undefined);
        assert.doesNotMatch(f.markup(), /role="alert"/); f.assertDraftPreserved();
      } finally { f.h.close(); }
    }
  }
});
