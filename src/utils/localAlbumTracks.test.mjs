import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createAlbumTracksLoader } from "./localAlbumTracks.ts";
import { LocalAlbumCatalog } from "./localAlbumCatalog.ts";

const tick = () => new Promise(resolve => setImmediate(resolve));
const album = id => ({ id, title: id, artist: "Artist", folder_path: id, discs: [], track_count: 1 });
const groups = id => [{ disc: { folder_path: id }, tracks: [{ path: `${id}/one.flac` }] }];

test("selection waits without queries until both managed Save and fresh catalog complete", async () => {
  let catalogLoads = 0, trackLoads = 0, state;
  const catalog = new LocalAlbumCatalog(async () => { catalogLoads++; return [album("kept")]; });
  const loader = createAlbumTracksLoader(async id => { trackLoads++; return groups(id); }, next => { state = next; });
  catalog.setUpdating(false, true); await tick();
  const old = catalog.getSnapshot().albums;
  catalog.setSaveBlocked(true); catalog.setUpdating(true);
  loader.select("kept", false);
  assert.equal(state.phase, "waiting"); assert.equal(trackLoads, 0);
  assert.equal(catalog.getSnapshot().albums, old);
  catalog.setUpdating(false); catalog.onMpdStatus(false); await tick();
  assert.equal(catalogLoads, 1, "MPD idle cannot release the Save gate");
  catalog.setSaveBlocked(false);
  assert.equal(catalog.getSnapshot().loading, true);
  await tick();
  loader.select("kept", !catalog.getSnapshot().loading && !catalog.getSnapshot().saveBlocked, catalog.getSnapshot().albums);
  await tick();
  assert.equal(trackLoads, 1); assert.equal(state.phase, "ready");
  assert.deepEqual(state.groups, groups("kept"));
  assert.equal(state.catalogVersion, catalog.getSnapshot().albums);
  assert.notEqual(state.catalogVersion, old, "tracks belong to the newly confirmed catalog");
});

test("album changes, update and removal discard stale track responses", async () => {
  const reads = [], states = [];
  const loader = createAlbumTracksLoader(id => new Promise(resolve => reads.push({ id, resolve })), state => states.push(state));
  loader.select("A", true); loader.select("B", true);
  reads[0].resolve(groups("A")); await tick();
  assert.equal(states.at(-1).albumId, "B"); assert.equal(states.at(-1).phase, "loading");
  reads[1].resolve(groups("B")); await tick(); assert.equal(states.at(-1).phase, "ready");
  loader.select("B", false); assert.deepEqual(states.at(-1).groups, []);
  loader.select("B", true); loader.select(null, true);
  reads[2].resolve(groups("B")); await tick();
  assert.equal(states.at(-1).albumId, null); assert.deepEqual(states.at(-1).groups, []);
});

test("exact MPD busy response waits for existing status polling; real errors remain errors", async () => {
  for (const [message, phase] of [
    ["Biblioteca local em atualização no MPD; tente novamente após a atualização.", "waiting"],
    ["ACK: query refused", "error"],
  ]) {
    let state, calls = 0, busy = 0;
    const loader = createAlbumTracksLoader(async () => { calls++; throw new Error(message); }, next => { state = next; }, () => busy++);
    loader.select("A", true); await tick(); await tick();
    assert.equal(calls, 1); assert.equal(state.phase, phase);
    assert.equal(busy, phase === "waiting" ? 1 : 0);
    assert.equal(state.error, phase === "error" ? message : null);
  }
});

test("busy recovery remains blocked by managed Save and discards in-flight catalog", async () => {
  const reads = [];
  const catalog = new LocalAlbumCatalog(() => new Promise(resolve => reads.push(resolve)));
  catalog.setUpdating(false, true); await tick();
  catalog.setSaveBlocked(true); reads[0]([album("unconfirmed")]); await tick();
  assert.deepEqual(catalog.getSnapshot().albums, []);
  catalog.waitForMpd(); catalog.onMpdStatus(false); await tick();
  assert.equal(reads.length, 1);
  catalog.setSaveBlocked(false); await tick();
  reads[1]([album("confirmed")]); await tick();
  assert.equal(catalog.getSnapshot().albums[0].id, "confirmed");
});

test("UI gates playback, selects only surviving albums and translates waiting/loading", () => {
  const view = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  assert.match(view, /catalog\.loaded[^;]*!catalog\.saveBlocked[^;]*!isLibraryUpdating/);
  assert.match(view, /tracksLoader\.select\(exists \? selectedAlbum\.id : null, catalogConfirmed, albums\)/);
  assert.match(view, /const tracksReady = catalogConfirmed && trackState\.catalogVersion === albums/);
  assert.match(view, /disabled=\{!isPlaybackAvailable \|\| !tracksReady\}/);
  assert.match(view, /if \(!isPlaybackAvailable \|\| !catalogConfirmed \|\| \(trackItems && !tracksReady\)\) return/);
  assert.match(view, /currentCatalog\.albums !== albums/);
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /if \(operation\.phase === "uncertain"\) localAlbumCatalog\.setSaveBlocked\(true\)/);
  assert.match(app, /operation\.phase === "completed"[\s\S]*?await configService\.getConfig\(\)[\s\S]*?localAlbumCatalog\.setSaveBlocked\(false\)/);
  for (const locale of ["en", "pt-BR"]) {
    const { localBrowser } = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    assert.ok(localBrowser.waitingTracks); assert.ok(localBrowser.loadingTracks);
  }
});
