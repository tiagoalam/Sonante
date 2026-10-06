import assert from "node:assert/strict";
import test from "node:test";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";
import {
  decadeForYear,
  deriveLocalYearNavigation,
  filterLocalAlbumsByYear,
  filterLocalAlbumsWithoutYear,
  validLocalYearSelection,
} from "./localYearNavigation.ts";

const album = (id, year, source_id = "Main") => ({
  id,
  title: `Album ${id}`,
  artist: "Artist",
  year,
  source_id,
  folder_path: `${source_id}/${id}`,
  track_count: 1,
  discs: [],
});

test("maps years to their mathematical decades", () => {
  assert.equal(decadeForYear(1974), 1970);
  assert.equal(decadeForYear(1999), 1990);
  assert.equal(decadeForYear(2000), 2000);
});

test("derives newest-first decades, counts, and ascending years", () => {
  const navigation = deriveLocalYearNavigation([
    album("a", "1974"), album("b", "1974"), album("c", "1979"), album("d", "2000"),
  ], 2026);
  assert.deepEqual(navigation.decades.map((item) => item.decade), [2000, 1970]);
  assert.equal(navigation.decades[1].albumCount, 3);
  assert.deepEqual(navigation.decades[1].years, [
    { year: 1974, albumCount: 2 },
    { year: 1979, albumCount: 1 },
  ]);
});

test("invalid, missing, and too-far future years are unknown", () => {
  const albums = [album("invalid", "20xx"), album("missing", undefined), album("future", "2028"), album("valid", "2027")];
  const navigation = deriveLocalYearNavigation(albums, 2026);
  assert.equal(navigation.unknownCount, 3);
  assert.deepEqual(filterLocalAlbumsWithoutYear(albums, 2026).map((item) => item.id), ["invalid", "missing", "future"]);
  assert.deepEqual(filterLocalAlbumsByYear(albums, 2027, 2026).map((item) => item.id), ["valid"]);
});

test("year navigation respects the source scope supplied by the caller", () => {
  const albums = [album("jazz", "1994", "Jazz"), album("rock", "2005", "Rock")];
  const scoped = filterLocalAlbumsByScope(albums, "Jazz", "");
  assert.deepEqual(deriveLocalYearNavigation(scoped, 2026).decades.map((item) => item.decade), [1990]);
});

test("source changes invalidate stale year selections at the nearest valid level", () => {
  const full = deriveLocalYearNavigation([album("a", "1994"), album("b", "1998")], 2026);
  assert.deepEqual(validLocalYearSelection(full, 1990, 1994, false), { decade: 1990, year: 1994, unknown: false });
  const sameDecade = deriveLocalYearNavigation([album("b", "1998")], 2026);
  assert.deepEqual(validLocalYearSelection(sameDecade, 1990, 1994, false), { decade: 1990, year: null, unknown: false });
  const absent = deriveLocalYearNavigation([album("c", "2005")], 2026);
  assert.deepEqual(validLocalYearSelection(absent, 1990, 1994, false), { decade: null, year: null, unknown: false });
  assert.deepEqual(validLocalYearSelection(absent, null, null, true), { decade: null, year: null, unknown: false });
});

test("ten thousand albums derive bounded decade and year summaries", () => {
  const albums = Array.from({ length: 10_000 }, (_, index) => album(String(index), String(1950 + (index % 77))));
  const navigation = deriveLocalYearNavigation(albums, 2026);
  assert.equal(navigation.decades.length, 8);
  assert.equal(navigation.decades.reduce((total, decade) => total + decade.albumCount, 0), 10_000);
  assert.ok(navigation.decades.every((decade) => decade.years.length <= 10));
});
