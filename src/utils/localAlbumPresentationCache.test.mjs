import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LocalAlbumCatalog, confirmedCatalogPlaybackAllowed, emptyLocalAlbumCatalogState, localCatalogPresentationAlbums } from "./localAlbumCatalog.ts";
import { artworkReadBlocked } from "./localArtwork.ts";
import { libraryDoctorAvailable } from "./libraryDoctor.ts";
import { createAlbumTracksLoader } from "./localAlbumTracks.ts";
import { createLocalAlbumPresentationCache, localAlbumPresentationContext,
  LOCAL_ALBUM_PRESENTATION_KEY, MAX_PRESENTATION_BYTES, MAX_PRESENTATION_ALBUMS,
  MAX_PRESENTATION_PREVIEW_ALBUMS, selectLocalAlbumPresentationPreview } from "./localAlbumPresentationCache.ts";
import { compareLocalAlbumsAscending, sortLocalAlbums } from "./localAlbumNavigation.ts";

const album = id => ({ id, title: "Álbum 🎵", artist: "Artist", year: "2001", genres: ["Jazz"],
  folder_path: `source/${id}`, source_id: "source", source_label: "Music", track_count: 2,
  discs: [{ number: 1, label: "Disc 1", folder_path: `source/${id}`, track_count: 2 }] });
const context = localAlbumPresentationContext(["/music"]);
const tick = () => new Promise(resolve => setImmediate(resolve));
function storageFixture() {
  let value = null, writes = 0, removes = 0;
  return {
    getItem: key => { assert.equal(key, LOCAL_ALBUM_PRESENTATION_KEY); return value; },
    setItem: (key, next) => { assert.equal(key, LOCAL_ALBUM_PRESENTATION_KEY); value = next; writes++; },
    removeItem: key => { assert.equal(key, LOCAL_ALBUM_PRESENTATION_KEY); value = null; removes++; },
    get value() { return value; }, get writes() { return writes; }, get removes() { return removes; },
  };
}
function fixture() {
  const storage = storageFixture();
  const cache = createLocalAlbumPresentationCache(() => storage);
  return { storage, cache };
}

test("one bounded allowlisted record survives a new session, without ancillary data", () => {
  const { storage, cache } = fixture();
  cache.write(context, [{ ...album("old"), thumb: "image bytes", tracks: ["audio"], plex_token: "secret",
    tag_presence: { evaluated_track_count: 2 }, discs: [{ ...album("old").discs[0], private: "extra" }] }]);
  assert.equal(storage.writes, 1);
  const reopened = createLocalAlbumPresentationCache(() => storage);
  assert.deepEqual(reopened.read(context), [album("old")]);
  assert.doesNotMatch(storage.value, /image bytes|audio|secret|tag_presence|private/);
  cache.write(context, [album("new")]);
  assert.deepEqual(reopened.read(context), [album("new")]);
});

test("restore paints rows synchronously without confirmation, reads, writes or capabilities", async () => {
  const { storage, cache } = fixture(); cache.write(context, [album("old")]);
  let loads = 0, trackReads = 0;
  const catalog = new LocalAlbumCatalog(async () => { loads++; return [album("fresh")]; }, cache);
  catalog.setPresentationContext(["/music"]);
  const state = catalog.getSnapshot();
  assert.equal(localCatalogPresentationAlbums(state)[0].id, "old"); assert.deepEqual(state.albums, []); assert.equal(state.loaded, false);
  assert.equal(state.loading, false); assert.equal(catalog.getConfirmedRevision(), 0);
  assert.equal(confirmedCatalogPlaybackAllowed(state, false), false);
  assert.equal(artworkReadBlocked(false, state), true);
  assert.equal(libraryDoctorAvailable(state, false, "old"), false);
  const tracks = createAlbumTracksLoader(async () => { trackReads++; return []; }, () => {});
  tracks.select("old", confirmedCatalogPlaybackAllowed(state, false), state.albums);
  catalog.setSaveBlocked(true); catalog.setConfirmedPlayback(true);
  assert.equal(confirmedCatalogPlaybackAllowed(catalog.getSnapshot(), true), false);
  await tick(); assert.equal(loads, 0); assert.equal(trackReads, 0); assert.equal(storage.writes, 1);
  catalog.setSaveBlocked(false); await tick();
  assert.equal(catalog.getSnapshot().loaded, true); assert.equal(catalog.getConfirmedRevision(), 1);
  assert.equal(catalog.getSnapshot().albums[0].id, "fresh");
  assert.equal(confirmedCatalogPlaybackAllowed(catalog.getSnapshot(), false), true);
  assert.equal(storage.writes, 2); assert.equal(cache.read(context)[0].id, "fresh");
});

test("cache miss/invalid storage cannot publish an empty confirmed library", async () => {
  for (const serialized of [null, "{", "null", "{}", JSON.stringify({ version: 1, context, albums: [] }), JSON.stringify({ version: 2, context, albums: [] }),
    JSON.stringify({ version: 1, context, albums: [{ ...album("bad"), discs: [{}] }] }),
    JSON.stringify({ version: 1, context, albums: [album("same"), album("same")] }),
    "x".repeat(MAX_PRESENTATION_BYTES + 1)]) {
    const storage = storageFixture(); if (serialized !== null) storage.setItem(LOCAL_ALBUM_PRESENTATION_KEY, serialized);
    const cache = createLocalAlbumPresentationCache(() => storage);
    const catalog = new LocalAlbumCatalog(async () => [], cache);
    catalog.setPresentationContext(["/music"]);
    assert.equal(catalog.getSnapshot().loaded, false);
    assert.deepEqual(catalog.getSnapshot().albums, []);
    assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "indexing");
    catalog.ensureLoaded(false, true); await tick();
    assert.equal(emptyLocalAlbumCatalogState(catalog.getSnapshot(), false), "empty");
  }
});

test("null optional native fields are accepted as presentation metadata", () => {
  const { cache } = fixture();
  cache.write(context, [{ ...album("native"), year: null, genres: null, source_label: null }]);
  const result = cache.read(context);
  assert.equal(result[0].id, "native"); assert.equal(result[0].year, undefined);
});

test("quota and storage access errors are optional and cannot suppress fresh results", async () => {
  for (const storage of [() => { throw Error("security"); }, () => ({ getItem() { throw Error("read"); },
    setItem() { throw Error("quota"); }, removeItem() { throw Error("security"); } })]) {
    const cache = createLocalAlbumPresentationCache(storage);
    const catalog = new LocalAlbumCatalog(async () => [album("fresh")], cache);
    assert.doesNotThrow(() => catalog.setPresentationContext(["/music"]));
    catalog.ensureLoaded(false, true); await tick();
    assert.equal(catalog.getSnapshot().error, null);
    assert.equal(catalog.getSnapshot().loaded, true);
    assert.equal(catalog.getSnapshot().albums[0].id, "fresh");
  }
});

test("context changes discard old rows and replies before publishing the next library", async () => {
  const { cache, storage } = fixture(); cache.write(context, [album("old")]);
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise(resolve => pending.push(resolve)), cache);
  catalog.ensureLoaded(false, true); await tick();
  assert.equal(pending.length, 0, "no catalog read before configuration binding");
  catalog.setPresentationContext(["/music"]); await tick();
  assert.equal(pending.length, 1);
  catalog.setPresentationContext(["/other"]);
  assert.deepEqual(catalog.getSnapshot().albums, []);
  assert.equal(catalog.getSnapshot().loaded, false);
  pending[0]([album("late-old-library")]); await tick();
  assert.equal(storage.writes, 1); assert.equal(pending.length, 2);
  assert.deepEqual(catalog.getSnapshot().albums, []);
  pending[1]([album("other")]); await tick();
  assert.equal(cache.read(context), null);
  assert.equal(cache.read(localAlbumPresentationContext(["/other"]))[0].id, "other");
  assert.equal(catalog.getConfirmedRevision(), 1);
});

test("a subscriber context change cannot write an accepted response into a different scope", async () => {
  const { cache, storage } = fixture();
  const pending = [];
  const catalog = new LocalAlbumCatalog(() => new Promise(resolve => pending.push(resolve)), cache);
  catalog.setPresentationContext(["/music"]);
  catalog.subscribe(() => {
    if (catalog.getSnapshot().loaded && catalog.getSnapshot().albums[0]?.id === "old") catalog.setPresentationContext(["/other"]);
  });
  catalog.ensureLoaded(false, true); await tick(); pending[0]([album("old")]); await tick();
  assert.equal(storage.writes, 0); assert.deepEqual(catalog.getSnapshot().albums, []);
  pending[1]([album("new")]); await tick();
  assert.equal(cache.read(localAlbumPresentationContext(["/other"]))[0].id, "new");
});

test("BUSY and errors keep provisional rows without confirming or overwriting them", async () => {
  const { cache, storage } = fixture(); cache.write(context, [album("old")]);
  let calls = 0;
  const catalog = new LocalAlbumCatalog(async () => {
    calls++; if (calls === 1) throw "SOURCE_AVAILABILITY_BUSY";
    throw Error("MPD unavailable");
  }, cache);
  catalog.setPresentationContext(["/music"]); catalog.ensureLoaded(false, true); await tick();
  assert.equal(catalog.getSnapshot().mpdBusy, true); assert.equal(catalog.getSnapshot().loaded, false);
  assert.equal(localCatalogPresentationAlbums(catalog.getSnapshot())[0].id, "old"); assert.equal(storage.writes, 1);
  catalog.onMpdStatus(false); await tick();
  assert.equal(catalog.getSnapshot().error, "MPD unavailable");
  assert.equal(catalog.getSnapshot().loaded, false); assert.equal(storage.writes, 1);
  assert.equal(catalog.getConfirmedRevision(), 0);
});

test("save/MPD invalidation never persists a late catalog", async () => {
  for (const invalidate of [c => c.setSaveBlocked(true), c => c.onMpdStatus(true)]) {
    const { cache, storage } = fixture(); cache.write(context, [album("old")]);
    let resolve;
    const catalog = new LocalAlbumCatalog(() => new Promise(r => { resolve = r; }), cache);
    catalog.setPresentationContext(["/music"]); catalog.ensureLoaded(false, true); await tick();
    invalidate(catalog); resolve([album("late")]); await tick();
    assert.equal(storage.writes, 1); assert.equal(catalog.getSnapshot().loaded, false);
    assert.equal(localCatalogPresentationAlbums(catalog.getSnapshot())[0].id, "old");
  }
});

test("oversized valid catalogs retain an exact opening prefix within the existing UTF-8 budget", () => {
  const { cache, storage } = fixture();
  const many = Array.from({ length: MAX_PRESENTATION_ALBUMS + 1 }, (_, i) => album(String(i))).reverse();
  cache.write(context, many);
  assert.equal(storage.writes, 1);
  assert.deepEqual(cache.read(context), sortLocalAlbums(many, "album-asc").slice(0, MAX_PRESENTATION_PREVIEW_ALBUMS));
  const large = Array.from({ length: 80 }, (_, i) => ({ ...album(String(i)), title: "🎵".repeat(4000), artist: "🎵".repeat(4000) })).reverse();
  const unicodeContext = localAlbumPresentationContext(['/música/"🎵"']);
  cache.write(unicodeContext, large);
  const restored = cache.read(unicodeContext);
  assert.ok(restored.length > 0 && restored.length < large.length);
  const ordered = sortLocalAlbums(large, "album-asc");
  assert.deepEqual(restored, ordered.slice(0, restored.length));
  assert.ok(Buffer.byteLength(storage.value, "utf8") <= MAX_PRESENTATION_BYTES);
  const next = JSON.stringify({ ...JSON.parse(storage.value), albums: [...restored, ordered[restored.length]] });
  assert.ok(Buffer.byteLength(next, "utf8") > MAX_PRESENTATION_BYTES);
  assert.equal(cache.read(context), null);
  assert.equal(localAlbumPresentationContext([]), null);
  assert.equal(localAlbumPresentationContext(["relative"]), null);
  assert.equal(localAlbumPresentationContext(["/" + "x".repeat(8192)]), null);
  assert.notEqual(localAlbumPresentationContext(["/a", "/b"]), localAlbumPresentationContext(["/b", "/a"]));
});

test("fitting snapshots keep every row and their original order, even above the preview count", () => {
  const { cache } = fixture();
  const fitting = Array.from({ length: 500 }, (_, i) => ({ ...album(String(i)), title: `Album ${500 - i}` }));
  cache.write(context, fitting);
  assert.deepEqual(cache.read(context), fitting);
  cache.write(context, []);
  assert.deepEqual(cache.read(context), []);
});

function largeMetadataCatalog(length) {
  const titles = ["Álbum 2", "Album 10", "álbum 2", "東京", 'A "quoted"'];
  return Array.from({ length }, (_, index) => {
    const i = length - index;
    return { ...album(`${i % 2 ? "ID" : "id"}-${i}`), title: titles[i % titles.length], artist: `Artist ${i % 11}`, year: String(1990 + i % 5) };
  });
}

test("50k/100k metadata previews match the grid order and selection comparison counts grow linearly", () => {
  const comparisons = [];
  for (const length of [50_000, 100_000]) {
    const input = largeMetadataCatalog(length);
    let calls = 0;
    const selected = selectLocalAlbumPresentationPreview(input, (left, right) => {
      calls++; return compareLocalAlbumsAscending(left, right);
    });
    // Full sort is only the independent test oracle; production sorts K rows.
    const expected = sortLocalAlbums(input, "album-asc").slice(0, MAX_PRESENTATION_PREVIEW_ALBUMS);
    assert.deepEqual(selected, expected);
    assert.ok(calls < length * (2 * Math.ceil(Math.log2(MAX_PRESENTATION_PREVIEW_ALBUMS)) + 3));
    comparisons.push(calls);
    const { cache, storage } = fixture();
    cache.write(context, input);
    assert.deepEqual(cache.read(context), expected);
    assert.ok(Buffer.byteLength(storage.value, "utf8") <= MAX_PRESENTATION_BYTES);
    let reads = 0, trackReads = 0;
    const catalog = new LocalAlbumCatalog(async () => { reads++; return []; }, cache);
    catalog.setPresentationContext(["/music"]);
    const state = catalog.getSnapshot();
    assert.deepEqual(localCatalogPresentationAlbums(state), expected);
    assert.deepEqual(state.albums, []);
    assert.equal(state.loaded, false);
    assert.equal(catalog.getConfirmedRevision(), 0);
    assert.equal(confirmedCatalogPlaybackAllowed(state, false), false);
    assert.equal(artworkReadBlocked(false, state), true);
    assert.equal(libraryDoctorAvailable(state, false, expected[0].id), false);
    const loader = createAlbumTracksLoader(async () => { trackReads++; return []; }, () => {});
    loader.select(expected[0].id, confirmedCatalogPlaybackAllowed(state, false), state.albums);
    assert.equal(reads, 0); assert.equal(trackReads, 0); assert.equal(storage.writes, 1);
  }
  assert.ok(comparisons[1] < comparisons[0] * 2.5, `comparison counts: ${comparisons}`);
});

test("invalid rows and duplicate IDs outside the retained preview reject the entire write", () => {
  const valid = Array.from({ length: MAX_PRESENTATION_ALBUMS + 1 }, (_, i) => album(String(i)));
  for (const tail of [
    { ...album("bad"), title: "x".repeat(8193) },
    { ...album("bad"), discs: [{}] },
    { ...album("bad"), genres: [null] },
    album(String(MAX_PRESENTATION_ALBUMS)),
  ]) {
    const { cache, storage } = fixture(); cache.write(context, [album("old")]);
    cache.write(context, [...valid, tail]);
    assert.equal(storage.writes, 1);
    assert.equal(storage.removes, 1);
    assert.equal(cache.read(context), null);
  }
  // Reading never salvages a partial record, even when its bytes fit.
  const { cache, storage } = fixture();
  storage.setItem(LOCAL_ALBUM_PRESENTATION_KEY, JSON.stringify({ version: 1, context,
    albums: Array.from({ length: MAX_PRESENTATION_ALBUMS + 1 }, (_, i) => ({ id: String(i), title: "A", artist: "", folder_path: "S", track_count: 1, discs: [] })) }));
  assert.ok(Buffer.byteLength(storage.value, "utf8") < MAX_PRESENTATION_BYTES);
  assert.equal(cache.read(context), null);
});

test("a first card that cannot fit is never skipped to cache a later card", () => {
  const { cache, storage } = fixture();
  const first = { ...album("first"), title: "A", discs: Array.from({ length: 256 }, () => ({
    number: 1, label: "🎵".repeat(4096), folder_path: "x".repeat(8192), track_count: 1,
  })) };
  cache.write(context, [{ ...album("later"), title: "Z" }, first]);
  assert.equal(storage.writes, 0);
  assert.equal(cache.read(context), null);
});

test("Local UI keeps saved cards inert, and configuration binding precedes React publication", () => {
  const source = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  const favorite = source.slice(source.indexOf("const handleToggleFavoriteLocal ="), source.indexOf("const handleSelectAlbum ="));
  let writes = 0;
  const toggle = new Function("localAlbumCatalog", "favoritesService", "setFavoriteIds", "logError", favorite.replace(/const handleToggleFavoriteLocal = async \(e: React.MouseEvent, album: LocalAlbum, cov: string \| null\)/,
    "const handleToggleFavoriteLocal = async (e, album, cov)").replace("const favItem: FavoriteAlbum", "const favItem") + "return handleToggleFavoriteLocal;");
  const call = toggle({ getSnapshot: () => ({ loaded: false, albums: [album("old")] }) }, { toggleFavorite: () => { writes++; } }, () => {}, () => {});
  void call({ stopPropagation() {} }, album("old"), null);
  assert.equal(writes, 0);
  assert.match(source, /disabled=\{!isPlaybackAvailable \|\| !readEnabled\}/);
  assert.match(source, /const albums = localCatalogPresentationAlbums\(catalog\)/);
  assert.match(source, /onClick=\{readEnabled \? onClick : undefined\}/);
  assert.equal((source.match(/readEnabled=\{!metadataMode && catalog.loaded\}/g) ?? []).length, 4);
  assert.match(source, /if \(!selectedAlbum \|\| !catalog.loaded \|\| metadataMode/);
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /localAlbumCatalog.setPresentationContext\(next.first_run \? \[\] : next.local_folders\);\s*setConfigState\(next\)/);
});


test("quota failure removes only the old optional record and fresh confirmation remains usable", async () => {
  const { storage, cache } = fixture(); cache.write(context, [album("old")]);
  storage.setItem = () => { throw Error("quota"); };
  const catalog = new LocalAlbumCatalog(async () => [album("fresh")], cache);
  catalog.setPresentationContext(["/music"]);
  assert.equal(localCatalogPresentationAlbums(catalog.getSnapshot())[0].id, "old");
  catalog.ensureLoaded(false, true); await tick();
  assert.equal(catalog.getSnapshot().loaded, true);
  assert.equal(catalog.getSnapshot().albums[0].id, "fresh");
  assert.equal(cache.read(context), null); assert.equal(storage.removes, 1);
});

test("reapplying the same folder context preserves the confirmed catalog and addition permission", async () => {
  const { cache } = fixture(); let loads = 0;
  const catalog = new LocalAlbumCatalog(async () => { loads++; return [album("confirmed")]; }, cache);
  catalog.setPresentationContext(["/music"]); catalog.ensureLoaded(false, true); await tick();
  const confirmed = catalog.getSnapshot().albums;
  catalog.setSaveBlocked(true); catalog.setConfirmedPlayback(true);
  catalog.setPresentationContext(["/music"]); await tick();
  assert.equal(catalog.getSnapshot().albums, confirmed); assert.equal(catalog.getSnapshot().loaded, true);
  assert.equal(confirmedCatalogPlaybackAllowed(catalog.getSnapshot(), true), true);
  assert.equal(loads, 1); assert.equal(catalog.getConfirmedRevision(), 1);
});


test("different configurations that exceed cache limits still invalidate confirmed state", async () => {
  const { cache, storage } = fixture(); let loads = 0;
  const catalog = new LocalAlbumCatalog(async () => { loads++; return [album(String(loads))]; }, cache);
  const unsupported = ["/" + "a".repeat(8192)];
  catalog.setPresentationContext(unsupported); catalog.ensureLoaded(false, true); await tick();
  assert.equal(catalog.getSnapshot().loaded, true); assert.equal(storage.writes, 0);
  catalog.setPresentationContext([...unsupported]); await tick(); assert.equal(loads, 1);
  catalog.setPresentationContext(["/" + "b".repeat(8192)]);
  assert.equal(catalog.getSnapshot().loaded, false); assert.deepEqual(catalog.getSnapshot().albums, []);
  await tick(); assert.equal(loads, 2); assert.equal(catalog.getSnapshot().albums[0].id, "2");
  assert.equal(storage.writes, 0);
});


test("added metadata survives reopening while legacy and malformed dates remain usable", () => {
  const { cache, storage } = fixture();
  const dated = { ...album("new"), added_at: "2026-10-30T12:00:00Z" };
  cache.write(context, [album("legacy"), dated, { ...album("invalid"), added_at: "2026-02-30T00:00:00Z" }]);
  const reopened = createLocalAlbumPresentationCache(() => storage).read(context);
  assert.deepEqual(reopened, [album("legacy"), dated, album("invalid")]);
  assert.deepEqual(sortLocalAlbums(reopened, "added-newest").map(a => a.id), ["new", "invalid", "legacy"]);
  assert.doesNotMatch(storage.value, /2026-02-30/);
});
