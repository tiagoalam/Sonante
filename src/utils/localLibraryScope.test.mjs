import assert from "node:assert/strict";
import test from "node:test";
import {
  filterLocalAlbumsByScope,
  localLibraryScopeResetKey,
  localLibrarySourceOptions,
  validLocalLibrarySourceSelection,
} from "./localLibraryScope.ts";
import { virtualGridMetrics } from "./virtualGrid.ts";

const album = (id, title, artist, source_id) => ({
  id,
  title,
  artist,
  source_id,
  folder_path: `${source_id ?? "Unknown"}/${id}`,
  track_count: 1,
  discs: [],
});

test("source options are deduplicated and sorted case-insensitively", () => {
  const options = localLibrarySourceOptions([
    album("1", "One", "Artist", "reggae"),
    album("2", "Two", "Artist", "Brasilidades"),
    album("3", "Three", "Artist", "Reggae"),
    album("4", "Four", "Artist", "reggae"),
  ]);
  assert.deepEqual(options.map(({ id }) => id), ["Brasilidades", "Reggae", "reggae"]);
});

test("All includes albums without a source", () => {
  const albums = [
    album("1", "Exodus", "Bob Marley", "Reggae"),
    album("2", "Loose", "Unknown", undefined),
  ];
  assert.deepEqual(filterLocalAlbumsByScope(albums, null, ""), albums);
});

test("a source includes only its albums and excludes unknown sources", () => {
  const albums = [
    album("1", "Exodus", "Bob Marley", "Reggae"),
    album("2", "Kind of Blue", "Miles Davis", "Jazz"),
    album("3", "Loose", "Unknown", undefined),
  ];
  assert.deepEqual(
    filterLocalAlbumsByScope(albums, "Reggae", "").map(({ id }) => id),
    ["1"],
  );
});

test("source and search filters are combined", () => {
  const albums = [
    album("1", "Exodus", "Bob Marley", "Reggae"),
    album("2", "Survival", "Bob Marley", "Reggae"),
    album("3", "Marley Covers", "Various", "Jazz"),
  ];
  assert.deepEqual(
    filterLocalAlbumsByScope(albums, "Reggae", "marley").map(({ id }) => id),
    ["1", "2"],
  );
});

test("a removed source invalidates the selection", () => {
  const options = localLibrarySourceOptions([
    album("1", "Album", "Artist", "Jazz"),
  ]);
  assert.equal(validLocalLibrarySourceSelection("Reggae", options), null);
  assert.equal(validLocalLibrarySourceSelection("Jazz", options), "Jazz");
});

test("scope and search both change the virtual grid reset key", () => {
  const initial = localLibraryScopeResetKey(null, "");
  assert.notEqual(localLibraryScopeResetKey("Reggae", ""), initial);
  assert.notEqual(
    localLibraryScopeResetKey("Reggae", "Marley"),
    localLibraryScopeResetKey("Reggae", ""),
  );
});

test("ten thousand albums filter as data while the mounted range stays small", () => {
  const albums = Array.from({ length: 10_000 }, (_, index) =>
    album(String(index), `Album ${index}`, "Artist", index % 2 ? "Jazz" : "Reggae"));
  const filtered = filterLocalAlbumsByScope(albums, "Reggae", "");
  const metrics = virtualGridMetrics({
    itemCount: filtered.length,
    containerWidth: 946,
    minimumColumnWidth: 170,
    gap: 24,
    cardExtraHeight: 64,
    scrollOffset: 50_000,
    viewportHeight: 700,
    overscanRows: 3,
  });
  assert.equal(filtered.length, 5_000);
  assert.ok((metrics.endRow - metrics.startRow) * metrics.columnCount <= 50);
});
