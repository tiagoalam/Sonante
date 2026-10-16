import test from "node:test";
import assert from "node:assert/strict";
import { groupExplorePresentation, explorePresentationArtistKey } from "./explorePresentation.ts";
import { createExploreSession, pickExploreSuggestions } from "./explore.ts";

const local = (id, title = "Pink Moon", artist = "Nick Drake", year) => {
  const album = { id, title, artist, year, folder_path: "Synthetic", discs: [], track_count: 1, genres: ["Rock"] };
  return { key: JSON.stringify(["local", id]), source: "local", album, version: [album] };
};
const copy = (id, title = "Pink Moon", artist = "Nick Drake", year) => local(id, title, artist, year);
const keys = entries => entries.map(entry => entry.key);

test("surface normalization groups copies and retains the deterministic Local representative, retaining context", () => {
  const l = local("L", "  POST  ", "BJO\u0308RK", "1995-06-13"), p = copy("P", "Post", "Björk", 1995);
  const before = JSON.stringify([l, p]);
  const groups = groupExplorePresentation([p, l]); assert.equal(groups.length, 1);
  assert.equal(groups[0].candidate, l); assert.deepEqual(groups[0].members, [p, l]);
  assert.deepEqual(pickExploreSuggestions([p, l], [p], () => 0), [p]);
  assert.equal(JSON.stringify([l, p]), before);
  assert.equal(explorePresentationArtistKey(l), explorePresentationArtistKey(p));
});

test("different artists, accents, punctuation and edition/disc/live qualifiers stay distinct", () => {
  const pool = [local("base"), copy("artist", "Pink Moon", "Nina Simone"),
    copy("deluxe", "Pink Moon (Deluxe)"), copy("remaster", "Pink Moon (2012 Remaster)"),
    copy("live", "Pink Moon Live"), copy("disc1", "Pink Moon — Disc 1"), copy("disc2", "Pink Moon — Disc 2"),
    copy("brackets", "Pink Moon [Deluxe]"), copy("accent", "Pink Moon", "Nìck Drake")];
  assert.equal(groupExplorePresentation(pool).length, pool.length);
});

test("known divergent years and ambiguous missing years cannot bridge conflicting editions", () => {
  const a = local("old", undefined, undefined, "1972"), b = copy("new", undefined, undefined, 2000);
  const unknown = local("missing"), missingCopy = copy("missing-copy");
  const pool = [a, unknown, b, missingCopy];
  const groups = groupExplorePresentation(pool); assert.equal(groups.length, 3);
  assert.equal(groups.find(group => group.members.includes(unknown)).candidate, unknown);
  assert.deepEqual(groups.find(group => group.members.includes(unknown)).members, [unknown, missingCopy]);
  assert.ok(groups.find(group => group.members.includes(a)).members.length === 1);
  assert.ok(groups.find(group => group.members.includes(b)).members.length === 1);
  assert.deepEqual(groupExplorePresentation([...pool].reverse()).map(g => [g.key, g.candidate.key]), groups.map(g => [g.key, g.candidate.key]));
  assert.equal(groupExplorePresentation([a, missingCopy]).length, 1);
  assert.equal(groupExplorePresentation([unknown, missingCopy]).length, 1);
  assert.equal(groupExplorePresentation([a, copy("same-date", undefined, undefined, 1972)]).length, 1);
});

test("absent, generic and symbol-only metadata leaves unrelated items separate", () => {
  for (const artist of ["", "  ", "Unknown", "Artista Desconhecido", "Various Artists", "Vários Artistas", "N/A", "???"]) {
    assert.equal(groupExplorePresentation([local("a", "Album", artist), copy("b", "Album", artist)]).length, 2, artist);
  }
  for (const title of ["", "Álbum Desconhecido", "Unknown album", "Untitled", "Not provided", "---"]) {
    assert.equal(groupExplorePresentation([local("a", title), copy("b", title)]).length, 2, title);
  }
  assert.equal(groupExplorePresentation([local("a", "Unknown Pleasures", "Joy Division"), copy("b", "Unknown Pleasures", "Joy Division")]).length, 1);
  for (const field of ["artist", "title"]) for (const value of [undefined, null, 42]) {
    const a = local("a"), b = copy("b"); a.album[field] = value; b.album[field] = value;
    assert.equal(groupExplorePresentation([a, b]).length, 2);
    assert.equal(pickExploreSuggestions([a, b], [], () => 0.5).length, 2);
  }
});

test("multiple copies contribute exactly one candidate and one sampling weight per presentation group", () => {
  const originals = Array.from({ length: 12 }, (_, i) => local(`L${i}`, `Album ${i}`, `Musician ${i}`, "2000"));
  const base = originals.flatMap((l, i) => [l, copy(`P${i}`, l.album.title, l.album.artist, 2000)]);
  const copies = [...base, ...originals.flatMap((l, i) => Array.from({ length: 20 }, (_, j) => copy(`copy${i}-${j}`, l.album.title, l.album.artist, 2000)))];
  assert.equal(groupExplorePresentation(base).length, 12); assert.equal(groupExplorePresentation(copies).length, 12);
  let drawsA = 0, drawsB = 0;
  const selectedA = pickExploreSuggestions(base, [], () => { drawsA++; return 0.3; });
  const selectedB = pickExploreSuggestions(copies, [], () => { drawsB++; return 0.3; });
  assert.equal(drawsA, 11); assert.equal(drawsB, 11); assert.deepEqual(selectedB, selectedA);
  assert.ok(selectedB.every(entry => originals.includes(entry))); assert.equal(selectedB.length, 10);
});

test("ten distinct groups fill ten cards; insufficient groups never pad with duplicates", () => {
  const ten = Array.from({ length: 10 }, (_, i) => local(`L${i}`, `Album ${i}`, `Musician ${i}`));
  const copies = ten.map((l, i) => copy(`P${i}`, l.album.title, l.album.artist));
  const selected = pickExploreSuggestions([...ten, ...copies], [], () => 0.5);
  assert.equal(selected.length, 10); assert.ok(selected.every(entry => entry.source === "local"));
  assert.equal(pickExploreSuggestions([ten[0], copies[0]], [], () => 0).length, 1);
  const mixed = [ten[0], ten[1], copy("unique1", "Other 1", "Other artist 1"), copy("unique2", "Other 2", "Other artist 2"), copies[0]];
  const diverse = pickExploreSuggestions(mixed, [], () => 0.5); assert.equal(diverse.length, 4);
  assert.deepEqual(new Set(diverse.map(e => e.source)), new Set(["local"]));
});







test("session renewal gives the same presentation selection and random-draw count with one or many Local copies", () => {
  const originals = Array.from({ length: 8 }, (_, i) => local(`L${i}`, `Album ${i}`, `Musician ${i}`));
  const singles = originals.map((l, i) => copy(`P${i}`, l.album.title, l.album.artist));
  const many = [...singles, ...originals.flatMap((l, i) => Array.from({ length: 15 }, (_, j) => copy(`copy${i}-${j}`, l.album.title, l.album.artist)))];
  const run = copies => {
    let seed = 17, draws = 0;
    const s = createExploreSession(() => { draws++; seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 2 ** 32; });
    s.syncContext({ local: { albums: [...originals, ...copies].map(e => e.album), state: "ready" } }, true);
    const rounds = [keys(s.visibleEntries())];
    for (let i = 0; i < 12; i++) { s.renew(); rounds.push(keys(s.visibleEntries())); }
    return { draws, rounds };
  };
  assert.deepEqual(run(many), run(singles));
});
