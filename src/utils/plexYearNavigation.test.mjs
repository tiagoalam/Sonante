import assert from "node:assert/strict";
import test from "node:test";
import { parsePlexAlbumYear } from "./plexAlbumNavigation.ts";
import {
  derivePlexYearNavigation,
  plexAlbumsForYear,
  plexAlbumsWithoutYear,
  plexDecadeForYear,
} from "./plexYearNavigation.ts";

const album = (rating_key, title, year) => ({ rating_key, title, artist: "Artist", year });

test("validates Plex years and maps decades", () => {
  assert.equal(parsePlexAlbumYear(1974, 2026), 1974);
  assert.equal(parsePlexAlbumYear(undefined, 2026), null);
  assert.equal(parsePlexAlbumYear(999, 2026), null);
  assert.equal(parsePlexAlbumYear(2028, 2026), null);
  assert.equal(parsePlexAlbumYear(2000.5, 2026), null);
  assert.equal(plexDecadeForYear(1974), 1970);
  assert.equal(plexDecadeForYear(2000), 2000);
});

test("derives newest-first decades, ascending present years, counts, and unknown", () => {
  const navigation = derivePlexYearNavigation([
    album("1", "One", 1999),
    album("2", "Two", 2000),
    album("3", "Three", 2002),
    album("4", "Four", 2000),
    album("5", "Five", undefined),
    album("6", "Six", 9),
  ], 2026);
  assert.deepEqual(navigation.decades, [
    { decade: 2000, albumCount: 3, years: [{ year: 2000, albumCount: 2 }, { year: 2002, albumCount: 1 }] },
    { decade: 1990, albumCount: 1, years: [{ year: 1999, albumCount: 1 }] },
  ]);
  assert.equal(navigation.unknownCount, 2);
});

test("filters one year and unknown albums with deterministic album A-Z", () => {
  const albums = [
    album("3", "Zulu", 1994),
    album("2", "África", 1994),
    album("1", "Missing B", undefined),
    album("0", "Missing A", 3000),
  ];
  assert.deepEqual(plexAlbumsForYear(albums, 1994, 2026).map(({ rating_key }) => rating_key), ["2", "3"]);
  assert.deepEqual(plexAlbumsWithoutYear(albums, 2026).map(({ rating_key }) => rating_key), ["0", "1"]);
});

test("derives 10000 albums without empty year entries", () => {
  const albums = Array.from({ length: 10_000 }, (_, index) =>
    album(String(index), `Album ${index}`, index % 11 === 0 ? undefined : 1980 + (index % 45)));
  const navigation = derivePlexYearNavigation(albums, 2026);
  assert.equal(navigation.decades.reduce((total, decade) => total + decade.albumCount, navigation.unknownCount), 10_000);
  assert.ok(navigation.decades.every((decade) => decade.years.every((year) => year.albumCount > 0)));
});
