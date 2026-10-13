import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { deriveAlbumGenres, albumGenreOptions, filterAlbumsByGenre, genreComparisonKey, validAlbumGenre } from "./albumGenres.ts";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";
import { sortLocalAlbums } from "./localAlbumNavigation.ts";
import { sortPlexAlbums } from "./plexAlbumNavigation.ts";
import ts from "typescript";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { localLibrarySelectorPresentation } from "./localLibraryScope.ts";

const localSource = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
const plexSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const compile = (expression, scope) => {
  const js = ts.transpileModule(`const result = (${expression});`, {
    compilerOptions: { target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.React },
  }).outputText;
  return new Function(...Object.keys(scope), `${js}; return result;`)(...Object.values(scope));
};
function initializer(source, name) {
  const tree = ts.createSourceFile("view.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let found;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(tree) === name) found = node.initializer.getText(tree);
    ts.forEachChild(node, visit);
  }
  visit(tree);
  assert.ok(found, name);
  return found;
}
const execute = (source, name, scope, ...args) => compile(initializer(source, name), scope)(...args);

const gridJs = ts.transpileModule(readFileSync(new URL("../components/AlbumGenreGrid.tsx", import.meta.url), "utf8"), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.React, module: ts.ModuleKind.CommonJS },
}).outputText;
const exports = {};
new Function("require", "exports", "React", gridJs)(() => ({ useTranslation: () => ({ t: key => key }) }), exports, React);
const { AlbumGenreGrid } = exports;

test("genre options preserve labels, deduplicate comparison keys and ignore invalid or legacy metadata", () => {
  const albums = [{ genres: ["Jazz", "Rock", "Música  Popular"] }, { genres: ["jazz", "mu\u0301sica popular"] }, {}, { genres: [null, 42, "", "bad\nvalue"] }, { genres: "Rock" }];
  assert.deepEqual(albumGenreOptions(albums), [
    { value: "jazz", label: "Jazz" }, { value: "música popular", label: "Música  Popular" }, { value: "rock", label: "Rock" },
  ]);
  assert.equal(genreComparisonKey(" JAzz "), "jazz");
  assert.deepEqual(filterAlbumsByGenre(albums, null), albums);
  assert.deepEqual(filterAlbumsByGenre(albums, "rock"), [albums[0]]);
});

test("Local multi-genre filtering composes with source, search and order without merging homonyms or CUE", () => {
  const albums = [
    { id: "A/cue", source_id: "A", title: "Same", artist: "Artist", genres: ["Rock", "Jazz"], folder_path: "A/Same", discs: [{ folder_path: "A/Same/image.cue" }] },
    { id: "B/album", source_id: "B", title: "Same", artist: "Artist", genres: ["Jazz"] },
    { id: "A/z", source_id: "A", title: "Zulu", artist: "Artist", genres: ["Rock"] },
    { id: "A/unknown", source_id: "A", title: "Old", artist: "Artist" },
  ];
  const matching = filterAlbumsByGenre(filterLocalAlbumsByScope(albums, "A", "artist"), "rock");
  assert.deepEqual(sortLocalAlbums(matching, "album-desc").map(album => album.id), ["A/z", "A/cue"]);
  assert.equal(matching[0], albums[0]);
  assert.equal(matching[0].discs[0].folder_path, "A/Same/image.cue");
  assert.deepEqual(filterAlbumsByGenre(filterLocalAlbumsByScope(albums, null, "same"), "jazz").map(album => album.id), ["A/cue", "B/album"]);
  assert.equal(albums[3].genres, undefined);
});

test("Plex genre filtering retains identity and base ordering; search results remain independently filterable", () => {
  const albums = [
    { rating_key: "3", title: "Same", artist: "B", genres: ["Rock", "Jazz"] },
    { rating_key: "1", title: "Same", artist: "A", genres: ["Jazz"] },
    { rating_key: "2", title: "Other", artist: "C" },
  ];
  assert.deepEqual(sortPlexAlbums(filterAlbumsByGenre(albums, "jazz"), "added_recent").map(album => album.rating_key), ["3", "1"]);
  assert.deepEqual(sortPlexAlbums(filterAlbumsByGenre(albums, "jazz"), "artist_asc").map(album => album.rating_key), ["1", "3"]);
  assert.deepEqual(filterAlbumsByGenre([albums[1]], "rock"), []);
  assert.deepEqual(albums.map(album => album.rating_key), ["3", "1", "2"]);
});

test("removed genres and changed library scopes reset selection without using the current search as the option set", () => {
  const valid = albumGenreOptions([{ genres: ["Jazz", "Rock"] }]);
  assert.equal(validAlbumGenre("jazz", valid), "jazz");
  assert.equal(validAlbumGenre(null, valid), null);
  assert.equal(validAlbumGenre("metal", valid), null);
  assert.equal(validAlbumGenre("jazz", albumGenreOptions([{ genres: ["Rock"] }])), null);
});

test("UI wiring restricts genres to album data and keeps confirmed source identities and virtual grids", () => {
  const local = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(local, /deriveLocalGenreFamilies\(scopedAlbums,/);
  assert.match(local, /\(\) => filterLocalAlbumsByScope\(scopedAlbums, null, albumSearch\),/);
  assert.match(local, /items=\{genreAlbums\}/);
  assert.match(local, /deriveLocalArtists\(scopedAlbums\)/);
  assert.match(local, /deriveLocalYearNavigation\(scopedAlbums\)/);
  assert.doesNotMatch(local, /common\.(?:allGenres|genreLabel)/);
  assert.match(local, /\["genres", Tags, t\("common\.genresLabel"\)\]/);
  assert.match(local, /localLibrarySelectorPresentation\(configuredLocalFolders\)/);
  assert.match(local, /libraryPresentation\.mode === "single" \?[\s\S]*?<span[\s\S]*?libraryPresentation\.label[\s\S]*?: <ToolbarSelect/);
  assert.match(local, /validLocalLibrarySourceSelection\(selectedSourceId, sourceOptions\)/);
  assert.match(app, /plexViewMode === "genres" \? validAlbumGenre/);
  assert.match(app, /\(\) => sortPlexAlbums\(albums, plexSortMode\)/);
  assert.doesNotMatch(app, /common\.(?:allGenres|genreLabel)/);
  assert.match(app, /items=\{selectedPlexGenreAlbums\}/);
  assert.match(app, /derivePlexArtists\(albums\)/);
  assert.match(app, /derivePlexYearNavigation\(albums\)/);
  assert.match(app, /visibleSearchAlbums\.map/);
  assert.equal((app.match(/configuredLocalFolders=\{config\?\.local_folders \?\? \[\]\}/g) ?? []).length, 3);
  assert.match(local, /items=\{sortedAlbums\}/);
  assert.match(app, /items=\{sortedPlexAlbums\}/);
});

test("genre labels resolve in pt-BR and English", () => {
  for (const [language, genre, all] of [["en", "Genre", "All genres"], ["pt-BR", "Gênero", "Todos os gêneros"]]) {
    const { common } = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8"));
    assert.equal(common.genreLabel, genre);
    assert.equal(common.allGenres, all);
  }
});

test("genre cards count each album once per normalized genre, retain homonyms and remain stably ordered", () => {
  const albums = [
    { id: "A/one", genres: ["Rock", "rock", " ROCK ", "Jazz"] },
    { id: "B/one", genres: ["Jazz", "Música  Popular"] },
    { id: "A/cue", genres: ["mu\u0301sica popular", "Rock"] },
    {}, { genres: [null, 42, ""] },
  ];
  assert.deepEqual(deriveAlbumGenres(albums), [
    { value: "jazz", label: "Jazz", albumCount: 2 },
    { value: "música popular", label: "Música  Popular", albumCount: 2 },
    { value: "rock", label: "Rock", albumCount: 2 },
  ]);
  assert.deepEqual(deriveAlbumGenres([]), []);
});

test("configured source presentation and genre scope support single, multiple and empty sources", () => {
  const albums = [
    { id: "A", source_id: "opaque-A", title: "Same", artist: "Artist", genres: ["Jazz"] },
    { id: "B", source_id: "opaque-B", title: "Same", artist: "Artist", genres: ["Rock", "Jazz"] },
  ];
  for (const [folders, mode] of [[[], "empty"], [["/fixture/A"], "single"], [["/fixture/A", "/fixture/Empty"], "multiple"]]) {
    assert.equal(localLibrarySelectorPresentation(folders).mode, mode);
  }
  const one = deriveAlbumGenres(filterLocalAlbumsByScope(albums, "opaque-A", ""));
  assert.deepEqual(one, [{ value: "jazz", label: "Jazz", albumCount: 1 }]);
  assert.equal(validAlbumGenre("rock", one), null);
  assert.equal(validAlbumGenre("jazz", one), "jazz");
  assert.equal(deriveAlbumGenres(filterLocalAlbumsByScope(albums, null, ""))[0].albumCount, 2);
  assert.deepEqual(deriveAlbumGenres(filterLocalAlbumsByScope(albums, "empty-source", "")), []);
});

function navigationFixture() {
  const state = {
    viewMode: "albums", plexViewMode: "albums", selectedGenre: null, selectedPlexGenre: null,
    selectedAlbum: null, selectedArtist: null, selectedDecade: null, selectedYear: null,
    unknownYearSelected: false, activeAlbum: null, activeArtist: null, activeCollection: null,
    fallbackPlexArtist: null, selectedPlexDecade: null, selectedPlexYear: null,
    selectedPlexUnknownYear: false, exploreDetail: null, selectedLibrary: { key: "one" },
  };
  const scope = () => ({
    ...state, activeGenre: state.selectedGenre, activePlexGenre: state.selectedPlexGenre,
    initialAlbumId: undefined, playerTarget: null, playerNavigationRequests: { invalidate() {} },
    plexLibraryRequestGenerationRef: { current: 1 }, plexSearchRequestGenerationRef: { current: 1 },
    libraries: [{ key: "one" }, { key: "two" }],
    onClearInitialArtist() {}, onPlayerNavigationBack() {}, setAlbumLocations() {}, setAlbumCover() {},
    setAlbums() {}, setCollections() {}, setHasCollections() {}, setLoading() {},
    setSearchQuery() {}, setSearchResults() {}, setPlayerNavigation() {}, setPlayerNavigationError() {}, setLocalSelectedArtist() {},
    ...Object.fromEntries(Object.keys(state).map(key => [`set${key[0].toUpperCase()}${key.slice(1)}`, value => { state[key] = value; }])),
  });
  return { state, scope, local: (name, ...args) => execute(localSource, name, scope(), ...args),
    plex: (name, ...args) => execute(plexSource, name, scope(), ...args) };
}

test("Local genre → album → back retains genre; second back and tab transitions clear only their context", async () => {
  const f = navigationFixture(), album = { id: "source/cue", title: "Same" };
  f.local("selectViewMode", "genres");
  f.state.selectedGenre = "jazz";
  await f.local("handleSelectAlbum", album);
  assert.equal(f.state.selectedAlbum, album);
  assert.equal(f.state.selectedGenre, "jazz");
  f.local("handleContextBack");
  assert.equal(f.state.selectedAlbum, null);
  assert.equal(f.state.selectedGenre, "jazz");
  assert.equal(f.state.viewMode, "genres");
  f.local("handleContextBack");
  assert.equal(f.state.selectedGenre, null);
  for (const mode of ["albums", "artists", "years", "folders", "genres"]) {
    f.state.selectedGenre = "jazz";
    f.local("selectViewMode", mode);
    assert.equal(f.state.viewMode, mode);
    assert.equal(f.state.selectedGenre, null);
  }
});

test("Plex album back retains genre, while genre back, mode changes and library changes invalidate it", () => {
  const f = navigationFixture();
  f.plex("handlePlexModeChange", "genres");
  f.state.selectedPlexGenre = "rock";
  f.state.activeAlbum = { rating_key: "real-album" };
  const albumBack = plexSource.match(/album=\{activeAlbum\}\s+onBack=\{([^}]+)\}/)[1];
  compile(albumBack, f.scope())();
  assert.equal(f.state.activeAlbum, null);
  assert.equal(f.state.selectedPlexGenre, "rock");
  assert.equal(f.state.plexViewMode, "genres");
  f.plex("handlePlexContextBack");
  assert.equal(f.state.selectedPlexGenre, null);
  for (const mode of ["albums", "artists", "years", "collections", "genres"]) {
    f.state.selectedPlexGenre = "rock";
    f.plex("handlePlexModeChange", mode);
    assert.equal(f.state.plexViewMode, mode);
    assert.equal(f.state.selectedPlexGenre, null);
  }
  f.state.selectedPlexGenre = "rock";
  f.plex("handlePlexLibraryChange", "two");
  assert.equal(f.state.selectedLibrary.key, "two");
  assert.equal(f.state.selectedPlexGenre, null);
  f.state.selectedPlexGenre = "rock";
  f.plex("resetAllNavigation");
  assert.equal(f.state.selectedPlexGenre, null);
});

test("actual selection expressions reject absent genres and never constrain the Albums tab", () => {
  for (const [source, variable, modeKey, selectedKey, optionsKey] of [
    [localSource, "activeGenre", "viewMode", "selectedGenre", "genreOptions"],
    [plexSource, "activePlexGenre", "plexViewMode", "selectedPlexGenre", "plexGenreOptions"],
  ]) {
    const expression = initializer(source, variable);
    const scope = { validAlbumGenre, [modeKey]: "genres", [selectedKey]: "rock", [optionsKey]: deriveAlbumGenres([{ genres: ["Rock"] }]) };
    assert.equal(compile(expression, scope), "rock");
    assert.equal(compile(expression, { ...scope, [optionsKey]: deriveAlbumGenres([{ genres: ["Jazz"] }]) }), null);
    assert.equal(compile(expression, { ...scope, [optionsKey]: [] }), null);
    assert.equal(compile(expression, { ...scope, [modeKey]: "albums" }), null);
  }
});

test("genre summary grid renders accessible buttons, counts and an explicit empty state", () => {
  let selected;
  const props = { genres: deriveAlbumGenres([{ genres: ["Rock", "Jazz"] }, { genres: ["jazz"] }]), onSelect: key => { selected = key; }, albumCountLabel: count => `count=${count}` };
  const element = AlbumGenreGrid(props);
  element.props.children[0].props.onClick();
  assert.equal(selected, "jazz");
  const markup = renderToStaticMarkup(element);
  assert.match(markup, /<button type="button"/);
  assert.match(markup, /Jazz/); assert.match(markup, /count=2/); assert.match(markup, /count=1/);
  assert.match(markup, /focus-visible:ring/);
  assert.doesNotMatch(markup, /<img|<a|<select/);
  assert.match(renderToStaticMarkup(AlbumGenreGrid({ ...props, genres: [] })), /common.emptyGenres/);
});

function renderGenreBranch(source, variable, end, bindings) {
  const start = `) : ${variable} === "genres" ? (`;
  const from = source.indexOf(start, source.indexOf(variable === "viewMode" ? 'ref={setScrollContainer}' : 'items={selectedPlexYearAlbums}'));
  assert.ok(from >= 0);
  const expression = source.slice(from + start.length, source.indexOf(end, from));
  return compile(expression, { React, AlbumGenreGrid, t: (key, values) => values?.count !== undefined ? `${key}:${values.count}` : key, ...bindings });
}

test("Local genre page reuses the virtual grid and real album identities, isolated from other views", () => {
  const album = { id: "opaque/cue", folder_path: "A/Album", genres: ["Jazz"] }, calls = [];
  const grid = props => { calls.push(props); return React.createElement("div", null, props.items.map(item => React.cloneElement(props.renderItem(item), { key: props.getItemKey(item) }))); };
  const base = { VirtualAlbumGrid: grid, LocalAlbumCard: props => React.createElement("span", null, props.album.id),
    activeGenre: "jazz", genreAlbums: [album], activeSourceId: "opaque", albumSearch: "", albumSortMode: "album-asc",
    scrollContainer: {}, favoriteIds: new Set(), handleToggleFavoriteLocal() {}, handleSelectAlbum() {}, handlePlayEntireAlbum() {},
    metadataMode: false, isPlaybackAvailable: true, onlineArtworkEnabled: false, isLibraryUpdating: false, emptyCatalogState: "ready", genreOptions: deriveAlbumGenres([album]), setSelectedGenre() {} };
  assert.match(renderToStaticMarkup(renderGenreBranch(localSource, "viewMode", ') : viewMode === "years" ? (', base)), /opaque\/cue/);
  assert.equal(calls[0].items[0], album);
  assert.equal(calls[0].getItemKey(album), album.id);
  const root = renderGenreBranch(localSource, "viewMode", ') : viewMode === "years" ? (', { ...base, activeGenre: null });
  assert.match(renderToStaticMarkup(root), /Jazz/);
  assert.equal(calls.length, 1, "summary cards do not render album cards or request their artwork");
  assert.match(renderToStaticMarkup(renderGenreBranch(localSource, "viewMode", ') : viewMode === "years" ? (', { ...base, genreAlbums: [] })), /localBrowser.emptyAlbums/);
});

test("Plex genre page reuses the virtual grid and rating keys, with no new IPC", () => {
  const album = { rating_key: "real-key", genres: ["Rock"] }, calls = [];
  const base = { activePlexGenre: "rock", selectedPlexGenreAlbums: [album], selectedLibrary: { key: "one" }, plexSortMode: "added_recent", plexScrollContainer: {},
    VirtualAlbumGrid: props => { calls.push(props); return React.createElement("div", null, props.items.map(item => React.cloneElement(props.renderItem(item), { key: props.getItemKey(item) }))); },
    PlexAlbumCard: props => React.createElement("span", null, props.album.rating_key), plexFavIds: new Set(), setActiveAlbum() {},
    handlePlayQuick() {}, handleTogglePlexCardFav() {}, handleSelectArtist() {}, isPlaybackAvailable: true,
    plexGenreOptions: deriveAlbumGenres([album]), setSelectedPlexGenre() {} };
  assert.match(renderToStaticMarkup(renderGenreBranch(plexSource, "plexViewMode", ') : plexViewMode === "albums" ? (', base)), /real-key/);
  assert.equal(calls[0].items[0], album);
  assert.equal(calls[0].getItemKey(album), album.rating_key);
  const root = renderGenreBranch(plexSource, "plexViewMode", ') : plexViewMode === "albums" ? (', { ...base, activePlexGenre: null });
  assert.match(renderToStaticMarkup(root), /Rock/);
  assert.equal(calls.length, 1);
});

test("genre navigation and missing metadata messages resolve in both supported languages", () => {
  for (const [language, genres, empty] of [["en", "Genres", "No genres found."], ["pt-BR", "Gêneros", "Nenhum gênero encontrado."]]) {
    const { common } = JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8"));
    assert.equal(common.genresLabel, genres);
    assert.equal(common.emptyGenres, empty);
  }
});
