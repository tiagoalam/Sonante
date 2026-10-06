import assert from "node:assert/strict";
import test from "node:test";
import {
  isTextPlexAlbumSort,
  parsePlexAlbumYear,
  plexAlbumBucket,
  plexAlbumBucketFirstIndices,
  plexAlbumNavigationResetKey,
  sortPlexAlbums,
} from "./plexAlbumNavigation.ts";

const album = (rating_key, title, artist, year) => ({ rating_key, title, artist, year });
const keys = (albums) => albums.map((item) => item.rating_key);

test("added_recent preserves the exact base order without mutating it", () => {
  const base = [album("3", "C", "Z", 2003), album("1", "A", "X", 2001), album("2", "B", "Y", 2002)];
  const sorted = sortPlexAlbums(base, "added_recent", 2026);
  assert.deepEqual(keys(sorted), ["3", "1", "2"]);
  assert.notEqual(sorted, base);
});

test("sorts album and artist text in both directions with numeric comparison", () => {
  const base = [
    album("10", "Album 10", "Queen", 2000),
    album("2", "Album 2", "Air", 2000),
    album("1", "A Night", "The Beatles", 2000),
  ];
  assert.deepEqual(keys(sortPlexAlbums(base, "album_asc", 2026)), ["1", "2", "10"]);
  assert.deepEqual(keys(sortPlexAlbums(base, "album_desc", 2026)), ["10", "2", "1"]);
  assert.deepEqual(keys(sortPlexAlbums(base, "artist_asc", 2026)), ["2", "10", "1"]);
  assert.deepEqual(keys(sortPlexAlbums(base, "artist_desc", 2026)), ["1", "10", "2"]);
});

test("sorts valid years both ways and always leaves invalid years last", () => {
  const base = [
    album("missing", "Missing", "A", undefined),
    album("invalid", "Invalid", "A", 999),
    album("future", "Future", "A", 2028),
    album("old", "Old", "A", 1970),
    album("new", "New", "A", 2020),
  ];
  assert.deepEqual(keys(sortPlexAlbums(base, "year_desc", 2026)), ["new", "old", "future", "invalid", "missing"]);
  assert.deepEqual(keys(sortPlexAlbums(base, "year_asc", 2026)), ["old", "new", "future", "invalid", "missing"]);
  assert.equal(parsePlexAlbumYear(2027, 2026), 2027);
  assert.equal(parsePlexAlbumYear(2028, 2026), null);
  assert.equal(parsePlexAlbumYear(2000.5, 2026), null);
});

test("ties are deterministic regardless of input order", () => {
  const base = [album("b", "Same", "Artist", 2000), album("a", "Same", "Artist", 2000)];
  assert.deepEqual(keys(sortPlexAlbums(base, "album_asc", 2026)), ["a", "b"]);
  assert.deepEqual(keys(sortPlexAlbums([...base].reverse(), "album_asc", 2026)), ["a", "b"]);
});

test("normalizes accented initials and assigns numbers and symbols to #", () => {
  assert.equal(plexAlbumBucket(album("a", "África", "X"), "album_asc"), "A");
  assert.equal(plexAlbumBucket(album("e", "É Tudo", "X"), "album_asc"), "E");
  assert.equal(plexAlbumBucket(album("c", "Çava", "X"), "album_asc"), "C");
  assert.equal(plexAlbumBucket(album("n", "123", "X"), "album_asc"), "#");
  assert.equal(plexAlbumBucket(album("s", "!Wow", "X"), "album_asc"), "#");
});

test("reports available buckets and their first indices", () => {
  const sorted = sortPlexAlbums([
    album("hash", "123", "X"), album("a1", "Alpha", "X"), album("a2", "Árvore", "X"), album("c", "Charlie", "X"),
  ], "album_asc", 2026);
  const indices = plexAlbumBucketFirstIndices(sorted, "album_asc");
  assert.equal(indices.get("#"), sorted.findIndex((item) => item.rating_key === "hash"));
  assert.equal(indices.get("A"), sorted.findIndex((item) => item.rating_key === "a1"));
  assert.equal(indices.get("C"), sorted.findIndex((item) => item.rating_key === "c"));
  assert.equal(indices.has("B"), false);
  for (const mode of ["album_asc", "album_desc", "artist_asc", "artist_desc"]) {
    assert.equal(isTextPlexAlbumSort(mode), true);
  }
  for (const mode of ["added_recent", "year_desc", "year_asc"]) {
    assert.equal(isTextPlexAlbumSort(mode), false);
  }
});

test("ten thousand albums remain data-only navigation input", () => {
  const base = Array.from({ length: 10_000 }, (_, index) => album(String(index), `Album ${10_000 - index}`, `Artist ${index % 100}`, 2000));
  const sorted = sortPlexAlbums(base, "album_asc", 2026);
  assert.equal(sorted.length, 10_000);
  assert.equal(plexAlbumBucketFirstIndices(sorted, "album_asc").get("A"), 0);
});

test("reset identity uses library key rather than display title", () => {
  const library = { key: "section-42", title: "Music" };
  assert.equal(plexAlbumNavigationResetKey(library.key, "album_asc"), '["section-42","album_asc"]');
  assert.notEqual(plexAlbumNavigationResetKey(library.key, "album_asc"), plexAlbumNavigationResetKey("Music", "album_asc"));
  assert.notEqual(plexAlbumNavigationResetKey(library.key, "album_asc"), plexAlbumNavigationResetKey(library.key, "artist_asc"));
});
