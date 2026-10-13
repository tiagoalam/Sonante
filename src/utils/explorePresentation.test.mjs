import test from "node:test";
import assert from "node:assert/strict";
import { groupExplorePresentation, explorePresentationArtistKey } from "./explorePresentation.ts";
import { createExploreSession, pickExploreSuggestions } from "./explore.ts";

const local = (id, title = "Pink Moon", artist = "Nick Drake", year) => {
  const album = { id, title, artist, year, folder_path: "Synthetic", discs: [], track_count: 1, genres: ["Rock"] };
  return { key: JSON.stringify(["local", id]), source: "local", album, version: [album] };
};
const plex = (id, title = "Pink Moon", artist = "Nick Drake", year, libraryId = "1") => {
  const album = { rating_key: id, title, artist, year, artist_rating_key: artist, genres: ["Rock"], library_id: libraryId };
  return { key: JSON.stringify(["plex", "server", libraryId, id]), source: "plex", album, version: [album], serverId: "server", libraryId, identity: {} };
};
const keys = entries => entries.map(entry => entry.key);

test("surface normalization groups copies and prefers the original eligible Local item, retaining context", () => {
  const l = local("L", "  POST  ", "BJO\u0308RK", "1995-06-13"), p = plex("P", "Post", "Björk", 1995);
  const before = JSON.stringify([l, p]);
  const groups = groupExplorePresentation([p, l]); assert.equal(groups.length, 1);
  assert.equal(groups[0].candidate, l); assert.deepEqual(groups[0].members, [p, l]);
  assert.deepEqual(pickExploreSuggestions([p, l], [p], () => 0), [l]);
  assert.equal(JSON.stringify([l, p]), before);
  assert.equal(explorePresentationArtistKey(l), explorePresentationArtistKey(p));
});

test("different artists, accents, punctuation and edition/disc/live qualifiers stay distinct", () => {
  const pool = [local("base"), plex("artist", "Pink Moon", "Nina Simone"),
    plex("deluxe", "Pink Moon (Deluxe)"), plex("remaster", "Pink Moon (2012 Remaster)"),
    plex("live", "Pink Moon Live"), plex("disc1", "Pink Moon — Disc 1"), plex("disc2", "Pink Moon — Disc 2"),
    plex("brackets", "Pink Moon [Deluxe]"), plex("accent", "Pink Moon", "Nìck Drake")];
  assert.equal(groupExplorePresentation(pool).length, pool.length);
});

test("known divergent years and ambiguous missing years cannot bridge conflicting editions", () => {
  const a = local("old", undefined, undefined, "1972"), b = plex("new", undefined, undefined, 2000);
  const unknown = local("missing"), copy = plex("missing-copy");
  const pool = [a, unknown, b, copy];
  const groups = groupExplorePresentation(pool); assert.equal(groups.length, 3);
  assert.equal(groups.find(group => group.members.includes(unknown)).candidate, unknown);
  assert.deepEqual(groups.find(group => group.members.includes(unknown)).members, [unknown, copy]);
  assert.ok(groups.find(group => group.members.includes(a)).members.length === 1);
  assert.ok(groups.find(group => group.members.includes(b)).members.length === 1);
  assert.deepEqual(groupExplorePresentation([...pool].reverse()).map(g => [g.key, g.candidate.key]), groups.map(g => [g.key, g.candidate.key]));
  assert.equal(groupExplorePresentation([a, copy]).length, 1);
  assert.equal(groupExplorePresentation([unknown, copy]).length, 1);
  assert.equal(groupExplorePresentation([a, plex("same-date", undefined, undefined, 1972)]).length, 1);
});

test("absent, generic and symbol-only metadata leaves unrelated items separate", () => {
  for (const artist of ["", "  ", "Unknown", "Artista Desconhecido", "Various Artists", "Vários Artistas", "N/A", "???"]) {
    assert.equal(groupExplorePresentation([local("a", "Album", artist), plex("b", "Album", artist)]).length, 2, artist);
  }
  for (const title of ["", "Álbum Desconhecido", "Unknown album", "Untitled", "Not provided", "---"]) {
    assert.equal(groupExplorePresentation([local("a", title), plex("b", title)]).length, 2, title);
  }
  assert.equal(groupExplorePresentation([local("a", "Unknown Pleasures", "Joy Division"), plex("b", "Unknown Pleasures", "Joy Division")]).length, 1);
  for (const field of ["artist", "title"]) for (const value of [undefined, null, 42]) {
    const a = local("a"), b = plex("b"); a.album[field] = value; b.album[field] = value;
    assert.equal(groupExplorePresentation([a, b]).length, 2);
    assert.equal(pickExploreSuggestions([a, b], [], () => 0.5).length, 2);
  }
});

test("multiple copies contribute exactly one candidate and one sampling weight per presentation group", () => {
  const originals = Array.from({ length: 12 }, (_, i) => local(`L${i}`, `Album ${i}`, `Musician ${i}`, "2000"));
  const base = originals.flatMap((l, i) => [l, plex(`P${i}`, l.album.title, l.album.artist, 2000)]);
  const copies = [...base, ...originals.flatMap((l, i) => Array.from({ length: 20 }, (_, j) => plex(`copy${i}-${j}`, l.album.title, l.album.artist, 2000)))];
  assert.equal(groupExplorePresentation(base).length, 12); assert.equal(groupExplorePresentation(copies).length, 12);
  let drawsA = 0, drawsB = 0;
  const selectedA = pickExploreSuggestions(base, [], () => { drawsA++; return 0.3; });
  const selectedB = pickExploreSuggestions(copies, [], () => { drawsB++; return 0.3; });
  assert.equal(drawsA, 11); assert.equal(drawsB, 11); assert.deepEqual(selectedB, selectedA);
  assert.ok(selectedB.every(entry => originals.includes(entry))); assert.equal(selectedB.length, 10);
});

test("ten distinct groups fill ten cards; insufficient groups never pad with copies or force Plex duplicates", () => {
  const ten = Array.from({ length: 10 }, (_, i) => local(`L${i}`, `Album ${i}`, `Musician ${i}`));
  const copies = ten.map((l, i) => plex(`P${i}`, l.album.title, l.album.artist));
  const selected = pickExploreSuggestions([...ten, ...copies], [], () => 0.5);
  assert.equal(selected.length, 10); assert.ok(selected.every(entry => entry.source === "local"));
  assert.equal(pickExploreSuggestions([ten[0], copies[0]], [], () => 0).length, 1);
  const mixed = [ten[0], ten[1], plex("unique1", "Other 1", "Other artist 1"), plex("unique2", "Other 2", "Other artist 2"), copies[0]];
  const diverse = pickExploreSuggestions(mixed, [], () => 0.5); assert.equal(diverse.length, 4);
  assert.deepEqual(new Set(diverse.map(e => e.source)), new Set(["local", "plex"]));
});

test("reconciliation removes an old repeated card and preserves the remaining valid representatives", () => {
  const l = local("L"), p = plex("P");
  const others = [local("a", "A", "A artist"), plex("b", "B", "B artist"), local("c", "C", "C artist"), plex("d", "D", "D artist")];
  const extra = plex("e", "E", "E artist");
  const old = [p, ...others, l];
  const selected = pickExploreSuggestions([l, p, ...others, extra], old, () => 0.5);
  assert.deepEqual(selected, [l, ...others, extra]);
});

test("Local availability falls back to Plex; restored Local preference applies at renewal", () => {
  const l = local("L"), p = plex("P"), s = createExploreSession(() => 0.5);
  const c = { local: { albums: [l.album], state: "ready" }, plex: { albums: [p.album], state: "ready", identity: {}, serverId: "server", libraryId: "1" } };
  s.syncContext(c, true); assert.equal(s.visibleEntries()[0].album, l.album); assert.equal(s.visibleEntries().length, 1);
  s.syncContext({ ...c, local: { ...c.local, state: "unavailable" } }, true);
  assert.equal(s.visibleEntries()[0].album, p.album); assert.equal(s.visibleEntries()[0].libraryId, "1");
  s.syncContext(c, true); assert.equal(s.visibleEntries()[0].album, p.album);
  s.renew(); assert.equal(s.visibleEntries()[0].album, l.album);
});

test("new library copies preserve the published cards; renewal samples groups and artist/genre slices retain the full catalog", () => {
  const locals = Array.from({ length: 3 }, (_, i) => local(`L${i}`, `Local ${i}`, `Local artist ${i}`));
  const one = Array.from({ length: 3 }, (_, i) => plex(`P${i}`, `Plex ${i}`, `Plex artist ${i}`));
  const localAlbums = locals.map(e => e.album), oneAlbums = one.map(e => e.album), identity = {};
  const c = { local: { albums: localAlbums, state: "ready" }, plex: { albums: oneAlbums, catalogs: [{ libraryId: "1", albums: oneAlbums }], state: "ready", identity, serverId: "server", libraryId: "" } };
  let draws = 0; const s = createExploreSession(() => { draws++; return 0.5; }); s.syncContext(c, true);
  const initial = keys(s.visibleEntries()), originalDraws = draws;
  for (let i = 0; i < 20; i++) s.syncContext({ ...c, local: { ...c.local }, plex: { ...c.plex } }, true);
  s.syncContext(c, false); s.syncContext(c, true); assert.equal(draws, originalDraws); assert.deepEqual(keys(s.visibleEntries()), initial);
  s.openSlice(s.getSnapshot().genres[0], s.getSnapshot().revision); assert.equal(s.getSnapshot().slice.entries.length, 6);
  const twoAlbums = [plex("local-copy", locals[0].album.title, locals[0].album.artist, undefined, "2").album,
    plex("plex-copy", one[0].album.title, one[0].album.artist, undefined, "2").album,
    plex("new", "New album", "New artist", undefined, "2").album];
  const next = { ...c, plex: { ...c.plex, albums: [...oneAlbums, ...twoAlbums], catalogs: [...c.plex.catalogs, { libraryId: "2", albums: twoAlbums }] } };
  s.syncContext(next, true); assert.deepEqual(keys(s.visibleEntries()), initial);
  assert.equal(s.candidateCount(), 9); assert.equal(s.getSnapshot().slice.entries.length, 9);
  s.leaveSlice(s.getSnapshot().revision); s.renew();
  assert.equal(s.visibleEntries().length, 7); assert.equal(groupExplorePresentation(s.visibleEntries()).length, 7);
  assert.ok(s.visibleEntries().some(e => e.source === "plex" && e.libraryId === "2" && e.album.rating_key === "new"));
  assert.equal(next.local.albums.length + next.plex.albums.length, 9);
  const artist = s.getSnapshot().artists.find(group => group.source === "local"); assert.ok(artist);
  const exact = localAlbums.find(album => album.artist === artist.name); assert.ok(exact);
  s.openSlice(artist, s.getSnapshot().revision); assert.equal(s.getSnapshot().slice.entries.length, 1);
  assert.equal(s.getSnapshot().slice.entries[0].album, exact);
});

test("session renewal gives the same presentation selection and random-draw count with one or many Plex copies", () => {
  const originals = Array.from({ length: 8 }, (_, i) => local(`L${i}`, `Album ${i}`, `Musician ${i}`));
  const singles = originals.map((l, i) => plex(`P${i}`, l.album.title, l.album.artist));
  const many = [...singles, ...originals.flatMap((l, i) => Array.from({ length: 15 }, (_, j) => plex(`copy${i}-${j}`, l.album.title, l.album.artist)))];
  const run = copies => {
    let seed = 17, draws = 0;
    const s = createExploreSession(() => { draws++; seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 2 ** 32; });
    s.syncContext({ local: { albums: originals.map(e => e.album), state: "ready" },
      plex: { albums: copies.map(e => e.album), state: "ready", identity: {}, serverId: "server", libraryId: "1" } }, true);
    const rounds = [keys(s.visibleEntries())];
    for (let i = 0; i < 12; i++) { s.renew(); rounds.push(keys(s.visibleEntries())); }
    return { draws, rounds };
  };
  assert.deepEqual(run(many), run(singles));
});


test("replacing a truly invalid item keeps a valid Plex card even after its Local copy arrives", () => {
  const p = plex("kept"), deleted = plex("deleted", "Other album", "Other artist");
  const identity = {}, c = { local: { albums: [], state: "empty" },
    plex: { albums: [p.album, deleted.album], state: "ready", identity, serverId: "server", libraryId: "1" } };
  const s = createExploreSession(() => 0.5); s.syncContext(c, true);
  const copy = local("copy"), alternative = local("new", "New album", "New artist");
  s.syncContext({ ...c, local: { albums: [copy.album, alternative.album], state: "ready" }, plex: { ...c.plex, albums: [p.album] } }, true);
  assert.deepEqual(new Set(s.visibleEntries().map(e => e.album)), new Set([p.album, alternative.album]));
  s.renew(); assert.ok(s.visibleEntries().some(e => e.album === copy.album));
});
