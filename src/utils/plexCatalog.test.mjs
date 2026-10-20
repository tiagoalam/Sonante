import { bindCollectionReset } from "../../tests/helpers/collectionResetFixture.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import ts from "typescript";
import { requireCompletePlexCatalog } from "./plexCatalog.ts";
import { PlexCatalogCoordinator } from "./plexCatalogCoordinator.ts";

test("browser adapter accepts only complete inventories for the requested library", () => {
  const albums = [{ rating_key: "1", title: "Album", artist: "Artist" }];
  const catalog = { server_id: "server", library_id: "1", completeness: "complete", total: 1, albums };
  assert.equal(requireCompletePlexCatalog(catalog, "1"), albums);
  assert.deepEqual(requireCompletePlexCatalog({ ...catalog, total: 0, albums: [] }, "1"), []);
  for (const change of [{ completeness: "partial" }, { completeness: "unknown" }, { library_id: "2" },
    { server_id: "" }, { total: 2 }, { total: -1 }, { total: 1.5 }, { albums: null }]) {
    assert.throws(() => requireCompletePlexCatalog({ ...catalog, ...change }, "1"), /completude ou contexto/);
  }
});

// Execute the actual result-application effect, with deferred IPC and no services.
const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const ast = ts.createSourceFile("App.tsx", app, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let effect, browserAlbumsExpression;
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
    && node.arguments[0].getText(ast).includes('plexCatalog.loadLibrary(selectedLibrary')) effect = node.arguments[0].getText(ast);
  if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "albums") browserAlbumsExpression = node.initializer.getText(ast);
  ts.forEachChild(node, visit);
}
visit(ast);
assert.ok(effect);
const compiled = ts.transpileModule(`const run = ${effect};`, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
assert.ok(browserAlbumsExpression);
const browserInventory = new Function("plexBrowserCatalog", "plexConnectionIdentity", "selectedLibrary", "loadedBrowserAlbums", `return (${browserAlbumsExpression});`);
const confirmedAlbums = snapshot => snapshot.libraries.filter(item => item.phase === "complete").flatMap(item => item.albums);
const flush = async () => { for (let i = 0; i < 16; i++) await Promise.resolve(); };

test("actual catalog effect preserves valid data on failure and drops library/logout-obsolete replies", async () => {
  for (const scenario of ["complete", "partial", "unknown", "http", "library", "logout", "account"]) {
    let resolve, reject;
    const pending = new Promise((a, b) => { resolve = a; reject = b; });
    const lastValid = [{ rating_key: "old", title: "Last valid", artist: "Artist" }];
    let inventory = lastValid;
    const states = [], diagnostics = [], connection = {}, identity = {}, generation = { current: 0 };
    const scope = { selectedLibrary: { key: "1" }, plexLibrariesIdentity: connection, plexConnectionIdentity: connection,
      centralSection: "plex", plexArea: "library", plexBrowserIdentity: identity, plexLibraryRequestGenerationRef: generation, plexAlbumLoad: { identity, phase: "ready" },
      setPlexAlbumLoad: value => states.push(value), setAlbums: value => { inventory = value; },
      plexService: { getCollections: async () => [] },
      plexCatalog: { loadLibrary: () => pending.then(catalog => requireCompletePlexCatalog(catalog, "1")) },
      logError: operation => diagnostics.push(operation) };
    for (const setter of ["setCollections", "setHasCollections", "setLoading", "setActiveAlbum", "setActiveCollection", "setActiveArtist", "setFallbackPlexArtist", "setSelectedPlexDecade", "setSelectedPlexYear", "setSelectedPlexUnknownYear", "setSelectedPlexGenre", "setSearchQuery", "setSearchResults", "setPlexViewMode"]) scope[setter] = () => {};
    const reset = bindCollectionReset(scope);
    const cleanup = new Function(...Object.keys(scope), `${compiled}; return run();`)(...Object.values(scope));
    reset.assertReset();
    assert.equal(inventory, lastValid);
    if (scenario === "library") generation.current++;
    if (["logout", "account"].includes(scenario)) { cleanup(); inventory = []; }
    const next = [{ rating_key: "new", title: "New", artist: "Artist" }];
    if (scenario === "http") reject({ completeness: "partial", received: 200, message: "HTTP 500" });
    else resolve({ server_id: "server", library_id: "1", completeness: ["partial", "unknown"].includes(scenario) ? scenario : "complete", total: 1, albums: next });
    await flush();
    if (scenario === "complete") { assert.equal(inventory, next); assert.equal(states.at(-1).phase, "ready"); }
    else if (["logout", "account"].includes(scenario)) { assert.deepEqual(inventory, []); assert.equal(states.at(-1).phase, "loading"); }
    else { assert.equal(inventory, lastValid); assert.equal(states.at(-1).phase, scenario === "library" ? "loading" : "error"); }
    assert.deepEqual(diagnostics, ["partial", "unknown", "http"].includes(scenario) ? ["plex.albums-load"] : []);
    cleanup();
  }
});

test("actual browser effect changes only its displayed library while concurrent per-library queries remain valid", async () => {
  const calls = [], identity = {}, connection = {}, generation = { current: 0 }, states = [];
  const coordinator = new PlexCatalogCoordinator(key => new Promise(resolve => { calls.push({ key, resolve }); }));
  coordinator.setContext(connection, "server");
  const libraries = [{ key: "1", title: "One" }, { key: "2", title: "Two" }];
  let displayed = [];
  const scope = { selectedLibrary: libraries[0], plexLibrariesIdentity: connection, plexConnectionIdentity: connection,
    centralSection: "plex", plexArea: "library", plexBrowserIdentity: identity, plexLibraryRequestGenerationRef: generation, plexAlbumLoad: null,
    setPlexAlbumLoad: value => states.push(value), setAlbums: value => { displayed = value; },
    plexCatalog: coordinator, plexService: { getCollections: async () => [] }, logError() {} };
  for (const setter of ["setCollections", "setHasCollections", "setLoading", "setActiveAlbum", "setActiveCollection", "setActiveArtist", "setFallbackPlexArtist", "setSelectedPlexDecade", "setSelectedPlexYear", "setSelectedPlexUnknownYear", "setSelectedPlexGenre", "setSearchQuery", "setSearchResults", "setPlexViewMode"]) scope[setter] = () => {};
  const reset = bindCollectionReset(scope);
  const run = () => new Function(...Object.keys(scope), `${compiled}; return run();`)(...Object.values(scope));
  const closeFirst = run(); reset.assertReset(); for (const library of libraries) void coordinator.loadLibrary(library).catch(() => {}); await flush(); assert.equal(calls.length, 2);
  closeFirst(); generation.current++; scope.selectedLibrary = libraries[1]; scope.plexBrowserIdentity = {};
  reset.state.active = { rating_key: "stale-second" }; reset.state.albums = [{ rating_key: "old-second" }]; reset.state.loading = true;
  const closeSecond = run(); reset.assertReset(2); await flush(); assert.equal(calls.length, 2);
  calls[0].resolve([{ rating_key: "a", title: "A", artist: "Artist" }]); await flush();
  assert.deepEqual(displayed, []); assert.equal(confirmedAlbums(coordinator.getSnapshot()).length, 1);
  calls[1].resolve([{ rating_key: "b", title: "B", artist: "Artist" }]); await flush();
  assert.equal(displayed[0].rating_key, "b"); assert.equal(states.at(-1).phase, "ready");
  assert.deepEqual(confirmedAlbums(coordinator.getSnapshot()).map(a => a.library_id), ["1", "2"]);
  closeSecond(); coordinator.invalidate();
});

test("browser reads a successful explicit browser retry immediately and masks inventories from a previous authentication context", async () => {
  const calls = [], identity = {};
  const c = new PlexCatalogCoordinator(key => new Promise((resolve, reject) => { calls.push({ key, resolve, reject }); }));
  c.setContext(identity, "server"); const libraries = [{ key: "1", title: "One" }, { key: "2", title: "Two" }];
  for (const library of libraries) void c.loadLibrary(library).catch(() => {}); await flush();
  calls[0].reject({ message: "HTTP 500" }); calls[1].resolve([{ rating_key: "other", title: "Other", artist: "Artist" }]); await flush();
  assert.deepEqual(browserInventory(c.getSnapshot(), identity, { key: "1" }, []), []);
  const retry = c.loadLibrary(libraries[0], true); await flush(); assert.equal(calls.length, 3);
  calls[2].resolve([{ rating_key: "retried", title: "Retried", artist: "Artist" }]); await retry; await flush();
  const snapshot = c.getSnapshot();
  assert.equal(browserInventory(snapshot, identity, { key: "1" }, [])[0].rating_key, "retried");
  const other = snapshot.libraries.find(item => item.library.key === "2").albums;
  assert.equal(browserInventory(snapshot, identity, { key: "2" }, []), other);
  assert.equal(await c.loadLibrary(libraries[1]), other); assert.equal(calls.length, 3, "retry cannot reload another confirmed library");
  assert.deepEqual(browserInventory(snapshot, {}, { key: "1" }, confirmedAlbums(snapshot)), []);
  c.invalidate();
});
