import assert from "node:assert/strict";
import test from "node:test";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";
import {
  albumsForLocalArtist,
  deriveLocalArtists,
  filterAndSortLocalArtists,
  localArtistBucketFirstIndices,
} from "./localArtistNavigation.ts";

const album = (id, artist, source_id = "Main") => ({
  id,
  title: `Album ${id}`,
  artist,
  source_id,
  folder_path: `${source_id}/${id}`,
  track_count: 1,
  discs: [],
});

test("derives artist album counts from the already scoped albums", () => {
  const all = [album("1", "Air", "Jazz"), album("2", "Air", "Jazz"), album("3", "Air", "Rock")];
  const scoped = filterLocalAlbumsByScope(all, "Jazz", "");
  assert.deepEqual(deriveLocalArtists(scoped), [{ id: "air", name: "Air", albumCount: 2 }]);
});

test("deduplicates case and accents with a deterministic display name", () => {
  const variants = [album("1", "João Gilberto"), album("2", "JOAO GILBERTO")];
  const forward = deriveLocalArtists(variants);
  const reverse = deriveLocalArtists([...variants].reverse());
  assert.equal(forward.length, 1);
  assert.deepEqual(forward, reverse);
  assert.equal(forward[0].albumCount, 2);
});

test("search is case and accent insensitive and sorting supports both directions", () => {
  const artists = deriveLocalArtists([
    album("1", "É Tudo"),
    album("2", "Air"),
    album("3", "B.B. King"),
  ]);
  assert.deepEqual(filterAndSortLocalArtists(artists, "e tu", "artist-asc").map((item) => item.name), ["É Tudo"]);
  assert.deepEqual(filterAndSortLocalArtists(artists, "", "artist-asc").map((item) => item.name), ["Air", "B.B. King", "É Tudo"]);
  assert.deepEqual(filterAndSortLocalArtists(artists, "", "artist-desc").map((item) => item.name), ["É Tudo", "B.B. King", "Air"]);
});

test("artist buckets expose first indices including #", () => {
  const artists = filterAndSortLocalArtists(deriveLocalArtists([
    album("1", "123"), album("2", "África"), album("3", "Air"), album("4", "Çava"),
  ]), "", "artist-asc");
  const buckets = localArtistBucketFirstIndices(artists);
  assert.equal(buckets.get("#"), artists.findIndex((item) => item.name === "123"));
  assert.equal(buckets.get("A"), artists.findIndex((item) => item.name === "África"));
  assert.equal(buckets.get("C"), artists.findIndex((item) => item.name === "Çava"));
  assert.equal(buckets.has("Z"), false);
});

test("artist discography uses the same normalized identity", () => {
  const albums = [album("1", "João Gilberto"), album("2", "JOAO GILBERTO"), album("3", "Air")];
  assert.deepEqual(albumsForLocalArtist(albums, "joao gilberto").map((item) => item.id), ["1", "2"]);
});

test("five thousand artists remain a data-only collection", () => {
  const albums = Array.from({ length: 10_000 }, (_, index) => album(String(index), `Artist ${index % 5_000}`));
  const artists = deriveLocalArtists(albums);
  const sorted = filterAndSortLocalArtists(artists, "", "artist-asc");
  assert.equal(sorted.length, 5_000);
  assert.equal(localArtistBucketFirstIndices(sorted).get("A"), 0);
});
