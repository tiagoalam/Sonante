import assert from "node:assert/strict";
import test from "node:test";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";
import {
  isTextLocalAlbumSort,
  localAlbumBucketFirstIndices,
  localAlbumIndexBucket,
  localAlbumNavigationResetKey,
  parseLocalAlbumYear,
  sortLocalAlbums,
} from "./localAlbumNavigation.ts";

const album = (id, title, artist, year, source_id = "Library") => ({
  id,
  title,
  artist,
  year,
  source_id,
  folder_path: `${source_id}/${id}`,
  track_count: 1,
  discs: [],
});

const ids = (albums) => albums.map(({ id }) => id);

test("sorts albums A-Z and Z-A with numeric text comparison", () => {
  const albums = [
    album("10", "Album 10", "B", "2000"),
    album("2", "Album 2", "A", "2000"),
    album("night", "A Night at the Opera", "Queen", "1975"),
  ];
  assert.deepEqual(ids(sortLocalAlbums(albums, "album-asc", 2026)), ["night", "2", "10"]);
  assert.deepEqual(ids(sortLocalAlbums(albums, "album-desc", 2026)), ["10", "2", "night"]);
});

test("text ordering is accent-insensitive before deterministic secondary keys", () => {
  const albums = [
    album("accented", "Álbum", "Zulu", "2000"),
    album("plain", "Album", "Alpha", "2000"),
  ];
  assert.deepEqual(ids(sortLocalAlbums(albums, "album-asc", 2026)), ["plain", "accented"]);
});

test("sorts artists A-Z and Z-A using title as the next key", () => {
  const albums = [
    album("queen-b", "B", "Queen", "1980"),
    album("beatles", "Abbey Road", "The Beatles", "1969"),
    album("queen-a", "A", "Queen", "1970"),
  ];
  assert.deepEqual(ids(sortLocalAlbums(albums, "artist-asc", 2026)), ["queen-a", "queen-b", "beatles"]);
  assert.deepEqual(ids(sortLocalAlbums(albums, "artist-desc", 2026)), ["beatles", "queen-b", "queen-a"]);
});

test("sorts valid years newest and oldest while invalid years always stay last", () => {
  const albums = [
    album("missing", "Missing", "A", undefined),
    album("invalid", "Invalid", "A", "20xx"),
    album("future", "Future", "A", "2028"),
    album("old", "Old", "A", "1970"),
    album("new", "New", "A", "2020"),
  ];
  assert.deepEqual(ids(sortLocalAlbums(albums, "year-newest", 2026)), ["new", "old", "future", "invalid", "missing"]);
  assert.deepEqual(ids(sortLocalAlbums(albums, "year-oldest", 2026)), ["old", "new", "future", "invalid", "missing"]);
  assert.equal(parseLocalAlbumYear("2027", 2026), 2027);
  assert.equal(parseLocalAlbumYear("2028", 2026), null);
  assert.equal(parseLocalAlbumYear(" 2020", 2026), null);
});

test("uses deterministic title and id tie breakers for equal years", () => {
  const albums = [
    album("b", "Same", "Z", "2000"),
    album("a", "Same", "A", "2000"),
    album("c", "Another", "Q", "2000"),
  ];
  assert.deepEqual(ids(sortLocalAlbums(albums, "year-newest", 2026)), ["c", "a", "b"]);
  assert.deepEqual(ids(sortLocalAlbums([...albums].reverse(), "year-newest", 2026)), ["c", "a", "b"]);
});

test("normalizes accented Latin initials and groups other initials in #", () => {
  assert.equal(localAlbumIndexBucket(album("a", "África Brasil", "X"), "album-asc"), "A");
  assert.equal(localAlbumIndexBucket(album("e", "É Tudo", "X"), "album-asc"), "E");
  assert.equal(localAlbumIndexBucket(album("c", "Çoração", "X"), "album-asc"), "C");
  assert.equal(localAlbumIndexBucket(album("n", "123", "X"), "album-asc"), "#");
  assert.equal(localAlbumIndexBucket(album("s", "!Wow", "X"), "album-asc"), "#");
  assert.equal(localAlbumIndexBucket(album("j", "東京", "X"), "album-asc"), "#");
});

test("artist sorts derive buckets from the displayed artist without special cases", () => {
  assert.equal(localAlbumIndexBucket(album("v", "Album", "Various Artists"), "artist-asc"), "V");
  assert.equal(localAlbumIndexBucket(album("u", "Album", "Artista Desconhecido"), "artist-desc"), "A");
  assert.equal(isTextLocalAlbumSort("year-newest"), false);
});

test("records only the first index for each available bucket", () => {
  const albums = [
    album("hash", "123", "X"),
    album("a1", "Alpha", "X"),
    album("a2", "Árvore", "X"),
    album("c", "Charlie", "X"),
  ];
  const indices = localAlbumBucketFirstIndices(albums, "album-asc");
  assert.equal(indices.get("#"), 0);
  assert.equal(indices.get("A"), 1);
  assert.equal(indices.get("C"), 3);
  assert.equal(indices.has("B"), false);
  assert.equal(localAlbumBucketFirstIndices(albums, "year-oldest").size, 0);
});

test("source, search, and sort form one coherent final list and reset key", () => {
  const albums = [
    album("z", "Survival", "Bob Marley", "1979", "Reggae"),
    album("a", "Exodus", "Bob Marley", "1977", "Reggae"),
    album("j", "Marley Covers", "Various", "2000", "Jazz"),
  ];
  const filtered = filterLocalAlbumsByScope(albums, "Reggae", "Marley");
  assert.deepEqual(ids(sortLocalAlbums(filtered, "album-asc", 2026)), ["a", "z"]);
  assert.notEqual(
    localAlbumNavigationResetKey("Reggae", "Marley", "album-asc"),
    localAlbumNavigationResetKey("Reggae", "Marley", "artist-asc"),
  );
  assert.notEqual(
    localAlbumNavigationResetKey(null, "Marley", "album-asc"),
    localAlbumNavigationResetKey("Reggae", "Marley", "album-asc"),
  );
  assert.notEqual(
    localAlbumNavigationResetKey("Reggae", "", "album-asc"),
    localAlbumNavigationResetKey("Reggae", "Marley", "album-asc"),
  );
});

test("ten thousand albums produce data-only sort and bucket indices", () => {
  const albums = Array.from({ length: 10_000 }, (_, index) =>
    album(String(index), `Album ${10_000 - index}`, `Artist ${index % 100}`, "2000"));
  const sorted = sortLocalAlbums(albums, "album-asc", 2026);
  const indices = localAlbumBucketFirstIndices(sorted, "album-asc");
  assert.equal(sorted.length, 10_000);
  assert.equal(indices.size, 1);
  assert.equal(indices.get("A"), 0);
});
