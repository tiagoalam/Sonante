import assert from "node:assert/strict";
import test from "node:test";
import {
  albumsForPlexArtist,
  derivePlexArtists,
  plexArtistBucketFirstIndices,
  sortPlexArtists,
} from "./plexArtistNavigation.ts";

const album = (rating_key, artist, artist_rating_key) => ({
  rating_key,
  title: `Album ${rating_key}`,
  artist,
  ...(artist_rating_key ? { artist_rating_key } : {}),
});

test("groups repeated Plex rating keys and counts albums", () => {
  const artists = derivePlexArtists([
    album("1", "Björk", "artist-1"),
    album("2", "BJÖRK", "artist-1"),
  ]);
  assert.deepEqual(artists, [{ id: "rating:artist-1", ratingKey: "artist-1", name: "BJÖRK", albumCount: 2 }]);
});

test("keeps different rating keys separate even when names match", () => {
  const artists = derivePlexArtists([
    album("1", "Phoenix", "a"),
    album("2", "Phoenix", "b"),
  ]);
  assert.deepEqual(artists.map(({ id, albumCount }) => ({ id, albumCount })), [
    { id: "rating:a", albumCount: 1 },
    { id: "rating:b", albumCount: 1 },
  ]);
});

test("groups missing rating keys by normalized name with deterministic display", () => {
  const artists = derivePlexArtists([
    album("1", "  João Gilberto  "),
    album("2", "JOAO GILBERTO"),
    album("3", "joão gilberto", "   "),
  ]);
  assert.deepEqual(artists, [{
    id: "name:joao gilberto",
    name: "JOAO GILBERTO",
    albumCount: 3,
  }]);
});

test("sorts artists A-Z and Z-A with deterministic id ties", () => {
  const artists = derivePlexArtists([
    album("1", "Zulu", "z"),
    album("2", "África", "a2"),
    album("3", "Africa", "a1"),
  ]);
  assert.deepEqual(sortPlexArtists(artists, "artist_asc").map(({ id }) => id), ["rating:a1", "rating:a2", "rating:z"]);
  assert.deepEqual(sortPlexArtists(artists, "artist_desc").map(({ id }) => id), ["rating:z", "rating:a2", "rating:a1"]);
});

test("builds accent-insensitive A-Z and # first indices", () => {
  const artists = sortPlexArtists(derivePlexArtists([
    album("1", "7 Seconds", "n"),
    album("2", "África", "a"),
    album("3", "É Tudo", "e"),
    album("4", "Çava", "c"),
  ]), "artist_asc");
  const indices = plexArtistBucketFirstIndices(artists);
  assert.equal(indices.get("#"), 0);
  assert.equal(indices.get("A"), 1);
  assert.equal(indices.get("C"), 2);
  assert.equal(indices.get("E"), 3);
  assert.equal(indices.has("B"), false);
});

test("derives fallback discography without inventing a rating key", () => {
  const albums = [album("1", "Air"), album("2", "AIR"), album("3", "Air", "rated")];
  const fallback = derivePlexArtists(albums).find(({ ratingKey }) => !ratingKey);
  assert.ok(fallback);
  assert.deepEqual(albumsForPlexArtist(albums, fallback).map(({ rating_key }) => rating_key), ["1", "2"]);
});

test("handles 10000 albums and 5000 artists", () => {
  const albums = Array.from({ length: 10_000 }, (_, index) =>
    album(String(index), `Artist ${index % 5000}`, `artist-${index % 5000}`));
  const artists = sortPlexArtists(derivePlexArtists(albums), "artist_asc");
  assert.equal(artists.length, 5000);
  assert.equal(artists.reduce((total, artist) => total + artist.albumCount, 0), 10_000);
  assert.equal(artists[0].name, "Artist 0");
});
