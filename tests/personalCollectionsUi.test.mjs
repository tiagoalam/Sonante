import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = file => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
const app = read("App.tsx");
const local = read("components/LocalBrowserView.tsx");
const plex = app.slice(app.indexOf("const PlexAlbumCard"), app.indexOf("PlexAlbumCard.displayName"));
const subsonic = read("components/NavidromeView.tsx");
const collections = read("components/PersonalCollectionsView.tsx");
const selection = read("components/PersonalCollectionSelection.tsx");
const service = read("services/sqliteLibrary.ts");

test("personal collections are a persisted main section gated by SQLite mode", () => {
  assert.match(app, /sqliteMode && <SidebarNavItem icon=\{Layers3\} label=\{t\("collections\.navTitle"\)\}/);
  assert.match(app, /centralSection === "collections" && sqliteMode \? <PersonalCollectionsView/);
  assert.match(app, /<PersonalCollectionSelectionProvider enabled=\{sqliteMode\}/);
});

test("source album cards collect only source-scoped identifiers and selection does not trigger album opening", () => {
  assert.match(local, /sqliteBacked && \(album\.id\.startsWith\("sqlite:"\) \|\| album\.id\.startsWith\("sqlite-cue-album:"\)\)/);
  assert.match(plex, /album\.server_id && album\.library_id[\s\S]*?ratingKey: album\.rating_key/);
  assert.match(subsonic, /connectionId, accountId: connection\.account_id, albumId: album\.id/);
  for (const source of [local, plex, subsonic]) assert.match(source, /event\.stopPropagation\(\); collectionSelection\.toggle\(collectionAlbum\)/);
  assert.match(selection, /personalCollectionAddLocal\(collectionId, album\.albumId\)/);
  assert.match(selection, /personalCollectionAddPlex\(collectionId, album\.serverId, album\.libraryId, album\.ratingKey\)/);
  assert.match(selection, /personalCollectionAddSubsonic\(collectionId, album\.connectionId, album\.accountId, album\.albumId\)/);
  assert.doesNotMatch(selection, /invoke\(|personal_collection_add_/);
  for (const method of ["personal_collection_add_local", "personal_collection_add_plex", "personal_collection_add_subsonic", "personal_collection_resolve"]) assert.ok(service.includes(`"${method}"`));
});

test("mixed collection management resolves on demand and keeps pending snapshots visible but non-openable", () => {
  assert.match(collections, /personalCollectionResolve\(selected\.id, member\.id\)/);
  assert.match(collections, /const canOpen = status === "available" && Boolean\(state\.result\?\.target\)/);
  assert.match(collections, /disabled=\{!canOpen\}/);
  assert.match(collections, /member\.title[\s\S]*?member\.artist/);
  assert.match(collections, /collections\.sources\.\$\{member\.source\}/);
  assert.match(collections, /status === "unresolved"[\s\S]*?collections\.status\.unresolvedHint/);
  assert.match(collections, /personalCollectionRemove\(selected\.id, member\.id\)/);
  assert.match(app, /if \(resolved\.source === "local"\)[\s\S]*?getLocalAlbumTracks\(album\.id\)/);
  assert.match(app, /kind: "plex", view: "album"/);
  assert.match(app, /kind: "navidrome", view: "album"/);
  assert.match(app, /collectionNavigationGeneration\.current\+\+; \}, \[mediaSource\]/);
  assert.match(collections, /onContextChange\?\.\(\)/);
  assert.match(collections, /previousSourceContext\.current === sourceContext/);
  assert.match(collections, /if \(selected\) void openCollection\(selected\)/);
  assert.match(app, /collectionSourceContext = useMemo\(\(\) => \(\{\}\), \[plexConnectionIdentity, navidromeConfiguration\.revision, localCatalog\.albums\]\)/);
  assert.doesNotMatch(collections, /personalCollectionRemove[\s\S]{0,150}(?:unavailable|unresolved)/);
});

test("collection list and detail expose CRUD while removal remains an explicit user action", () => {
  assert.match(collections, /personalCollectionCreate\(value\)/);
  assert.match(collections, /window\.prompt\(t\("collections\.renamePrompt"\), selected\.name\)/);
  assert.match(collections, /personalCollectionRename\(selected\.id, value\)/);
  assert.match(collections, /window\.confirm\(t\("collections\.deleteConfirm"/);
  assert.match(collections, /personalCollectionDelete\(id\)/);
  assert.match(collections, /personalCollectionRemove\(selected\.id, member\.id\)/);
  assert.match(collections, /personalCollectionMembers\(collection\.id\)/);
  assert.match(collections, /collections\.albumCount/);
});

test("collection states and selection feedback are translated in English and Brazilian Portuguese", () => {
  for (const locale of ["en", "pt-BR"]) {
    const dictionary = JSON.parse(readFileSync(new URL(`../src/locales/${locale}.json`, import.meta.url), "utf8")).collections;
    for (const key of ["navTitle", "title", "description", "selectedCount_one", "selectedCount_other", "addToCollection", "selectAlbum", "deselectAlbum", "clearSelection", "close", "empty", "emptyMembers", "resultSummary", "sources", "status", "errors"]) assert.ok(dictionary[key], `${locale}: ${key}`);
    for (const status of ["available", "unavailable", "unresolved", "checkFailed"]) assert.ok(dictionary.status[status], `${locale}: status.${status}`);
  }
});

test("dialog count describes the displayed attempt after partial success", () => {
  assert.match(selection, /counts\.failed \? t\("collections\.partialResult"\)/);
  assert.match(selection, /retry && previous \? mergeCollectionOutcomes/);
});

test("result dialog keeps retry, close, navigation and safe keyboard paths", () => {
  assert.match(selection, /event\.key === "Escape" && !busyRef\.current/);
  assert.match(selection, /previous\?\.focus\(\)/);
  assert.match(selection, /associate\(destination, false, true\)/);
  assert.match(selection, /close\(\); onViewCollection\(id\)/);
  assert.match(selection, /t\("collections\.done"\)/);
  assert.match(selection, /disabled=\{busy\} onClick=\{close\}/);
  assert.match(collections, /requestedCollectionId[\s\S]*?openCollection\(requested\)/);
  for (const locale of ["en", "pt-BR"]) {
    const c = JSON.parse(readFileSync(new URL(`../src/locales/${locale}.json`, import.meta.url), "utf8")).collections;
    for (const key of ["addedResult_one", "addedResult_other", "notAdded_one", "notAdded_other", "duplicateCount_one", "duplicateCount_other", "retryFailed", "viewCollection", "done"]) assert.ok(c[key], `${locale}: ${key}`);
  }
});
