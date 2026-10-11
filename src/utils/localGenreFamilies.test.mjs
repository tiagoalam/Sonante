import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import i18next from "i18next";
import { classifyLocalAlbumFamily, deriveLocalGenreFamilies, filterLocalAlbumsByFamily } from "./localGenreFamilies.ts";
import { deriveAlbumGenres, filterAlbumsByGenre, validAlbumGenre } from "./albumGenres.ts";
import { filterLocalAlbumsByScope } from "./localLibraryScope.ts";
import { sortLocalAlbums } from "./localAlbumNavigation.ts";

const classify = (...genres) => classifyLocalAlbumFamily({ genres });
const resources = Object.fromEntries(["en", "pt-BR"].map(language => [language, {
  translation: JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8")),
}]));
const label = family => resources["pt-BR"].translation.localGenreFamilies[family];

test("requested compound examples classify by exact terms and documented priority", () => {
  for (const [tag, family] of [
    ["Art Rock", "rock"], ["Hard Bop", "jazz"], ["Chicago Blues", "blues"],
    ["Big Beat/Hip Hop/Breakbeat Hardcore", "electronic"],
    ["Blues, Folk, World, & Country", "blues"],
  ]) assert.equal(classify(tag), family, tag);
});

test("broad families recognize explicit aliases and accent/case/spacing without rewriting them", () => {
  for (const [tag, family] of [
    ["Heavy Metal", "metal"], ["Techno", "electronic"], ["Neo-Soul", "funkSoul"],
    ["Funk / Soul", "funkSoul"], ["Rhythm and Blues", "funkSoul"], ["Drum & Bass", "electronic"],
    ["Folk / Country", "folkCountry"], ["Singer-Songwriter", "folkCountry"], ["Hip-Hop", "hipHop"],
    ["Pop", "pop"], ["Roots Reggae", "reggae"], ["Mu\u0301sica  Brasileira", "brazilian"],
    ["Bossa Nova", "brazilian"], ["Funk Carioca", "brazilian"], ["Clássica", "classical"],
    ["Salsa", "latin"], ["Fado", "world"], ["Trilha Sonora", "soundtrack"],
    ["  aRt   ROCK  ", "rock"],
  ]) assert.equal(classify(tag), family, tag);
});

test("unknown, incomplete and ambiguous tags cannot match arbitrary substrings", () => {
  for (const tag of [
    "", "Rock-ish", "Not Rock", "Hard Bop-ish", "Jazzercise", "Popcorn", "Classical-ish",
    "alternative", "fusion", "dance", "psychedelic", "symphonic", "Rock / Unknown", "Jazz, Something Else",
    "Pop;;Rock", "Rock/", "/Rock", "Hard-Bop", "Blues Folk World Country", "Rock/42",
    "Rock\n", "Rock\u0000", "Rock\u007f", "Rock\u0085", "x".repeat(4097),
  ]) assert.equal(classify(tag), "other", tag);
  for (const album of [{}, { genres: [] }, { genres: null }, { genres: "Jazz" }, { genres: [null, 42, {}] }]) {
    assert.equal(classifyLocalAlbumFamily(album), "other");
  }
});

test("cross-family combinations use the same fixed priority for delimiters and multiple tags", () => {
  for (const [genres, expected] of [
    [["Rock", "Pop"], "rock"], [["Jazz", "Rock"], "jazz"], [["Heavy Metal", "Rock"], "metal"],
    [["Hip Hop", "Big Beat"], "electronic"], [["Folk", "World", "Country", "Blues"], "blues"],
    [["Funk", "MPB"], "brazilian"], [["Pop", "Opera"], "classical"], [["Rock", "Roots Reggae"], "reggae"],
    [["Jazz", "unknown", "Rock/Unknown"], "jazz"],
  ]) {
    assert.equal(classify(...genres), expected);
    assert.equal(classify(...genres.toReversed()), expected, "metadata order is irrelevant");
    assert.equal(classify(...genres, ...genres, ...genres), expected, "repetition is not a vote");
  }
  for (const value of ["Rock/Pop", "Pop,Rock", "Pop;Rock", "Pop , & Rock"]) assert.equal(classify(value), "rock");
  for (const value of ["Funk/Soul", "Folk/ Country", "Drum & Bass/Hip-Hop"]) assert.notEqual(classify(value), "other");
});

const albums = Object.freeze([
  Object.freeze({ id: "A/cue", source_id: "opaque-A", title: "Same", artist: "Artist", folder_path: "A/Album", discs: Object.freeze([{ folder_path: "A/Album/image.cue" }]), genres: Object.freeze(["Art Rock", "Pop", "rock"]) }),
  Object.freeze({ id: "B/album", source_id: "opaque-B", title: "Same", artist: "Artist", genres: Object.freeze(["Hard Bop", "Jazz", "Rock"]) }),
  Object.freeze({ id: "A/blues", source_id: "opaque-A", title: "Zulu", artist: "Artist", genres: Object.freeze(["Chicago Blues", "Folk"]) }),
  Object.freeze({ id: "A/brazil", source_id: "opaque-A", title: "Brasil", artist: "Artist", genres: Object.freeze(["Samba", "Soul"]) }),
  Object.freeze({ id: "A/unknown", source_id: "opaque-A", title: "Unknown", artist: "Artist" }),
]);

test("every album belongs to exactly one card and its matching grid, preserving all original identities and tags", () => {
  const before = JSON.stringify(albums);
  const summaries = deriveLocalGenreFamilies(albums, label);
  assert.equal(summaries.reduce((sum, family) => sum + family.albumCount, 0), albums.length);
  const ids = [];
  for (const family of summaries) {
    const matching = filterLocalAlbumsByFamily(albums, family.value);
    assert.equal(matching.length, family.albumCount);
    assert.ok(matching.every(album => albums.includes(album)), "original objects reach album navigation and playback");
    ids.push(...matching.map(album => album.id));
  }
  assert.equal(new Set(ids).size, albums.length);
  assert.deepEqual(ids.toSorted(), albums.map(album => album.id).toSorted());
  assert.equal(filterLocalAlbumsByFamily(albums, "rock")[0].discs[0].folder_path, "A/Album/image.cue");
  assert.deepEqual(filterLocalAlbumsByFamily(albums, "not-a-family"), []);
  assert.equal(JSON.stringify(albums), before);
});

test("family navigation combines source, search and sort while absent selections reset", () => {
  const a = filterLocalAlbumsByScope(albums, "opaque-A", "");
  const summaries = deriveLocalGenreFamilies(a, label);
  assert.equal(validAlbumGenre("jazz", summaries), null);
  assert.equal(validAlbumGenre("rock", summaries), "rock");
  assert.deepEqual(filterLocalAlbumsByFamily(filterLocalAlbumsByScope(a, null, "same"), "rock").map(album => album.id), ["A/cue"]);
  assert.deepEqual(filterLocalAlbumsByFamily(filterLocalAlbumsByScope(a, null, "no match"), "rock"), []);
  assert.deepEqual(sortLocalAlbums(filterLocalAlbumsByFamily(albums, "jazz"), "album-desc").map(album => album.id), ["B/album"]);
  assert.deepEqual(deriveLocalGenreFamilies([], label), []);
  assert.equal(deriveLocalGenreFamilies(a, label).at(-1).value, "other");
  assert.deepEqual(deriveLocalGenreFamilies(albums, label).map(family => family.value), deriveLocalGenreFamilies(albums.toReversed(), label).map(family => family.value));
});

test("Plex still lists raw genres, counts albums in multiple genres and retains original labels", () => {
  const plex = [{ rating_key: "1", genres: ["Art Rock", "Jazz"] }, { rating_key: "2", genres: ["Rock"] }];
  assert.deepEqual(deriveAlbumGenres(plex), [
    { value: "art rock", label: "Art Rock", albumCount: 1 },
    { value: "jazz", label: "Jazz", albumCount: 1 },
    { value: "rock", label: "Rock", albumCount: 1 },
  ]);
  assert.equal(filterAlbumsByGenre(plex, "jazz")[0], plex[0]);
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /deriveAlbumGenres\(albums\)/);
  assert.doesNotMatch(app, /localGenreFamilies|filterLocalAlbumsByFamily/);
});

test("all family labels resolve in pt-BR and en, without coupling identity to language", async () => {
  const everyFamily = ["Samba", "Opera", "Heavy Metal", "Techno", "Jazz", "Blues", "Reggae", "Rap", "Soul", "Folk", "Rock", "Pop", "Salsa", "World", "Score", "Unknown"].map(genre => ({ genres: [genre] }));
  const keys = Object.keys(resources.en.translation.localGenreFamilies).sort();
  assert.deepEqual(keys, Object.keys(resources["pt-BR"].translation.localGenreFamilies).sort());
  for (const language of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng: language, resources, fallbackLng: false });
    const summaries = deriveLocalGenreFamilies(everyFamily, family => instance.t(`localGenreFamilies.${family}`));
    assert.equal(summaries.length, 16);
    assert.deepEqual(summaries.map(family => family.value).sort(), keys);
    assert.ok(summaries.every(family => family.label && !family.label.startsWith("localGenreFamilies.")));
    assert.equal(summaries.at(-1).label, language === "pt-BR" ? "Outros" : "Others");
    assert.equal(summaries.find(family => family.value === "brazilian").label, language === "pt-BR" ? "Música Brasileira" : "Brazilian Music");
  }
});

test("the actual Local view derives translated family cards and filters their album grid from the same classification", () => {
  const source = readFileSync(new URL("../components/LocalBrowserView.tsx", import.meta.url), "utf8");
  const tree = ts.createSourceFile("view.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const initializers = new Map();
  function visit(node) {
    if (ts.isVariableDeclaration(node)) initializers.set(node.name.getText(tree), node.initializer?.getText(tree));
    ts.forEachChild(node, visit);
  }
  visit(tree);
  const evaluate = (name, scope) => {
    const js = ts.transpileModule(`const result = (${initializers.get(name)});`, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
    return new Function(...Object.keys(scope), `${js}; return result;`)(...Object.values(scope));
  };
  const scope = { useMemo: fn => fn(), deriveLocalGenreFamilies, filterLocalAlbumsByFamily, sortLocalAlbums, filterLocalAlbumsByScope,
    scopedAlbums: albums, albumSearch: "", albumSortMode: "album-asc", activeGenre: "jazz", t: key => resources["pt-BR"].translation.localGenreFamilies[key.split(".")[1]] };
  scope.genreFamilyLabels = evaluate("genreFamilyLabels", scope);
  const options = evaluate("genreOptions", scope);
  assert.equal(options.find(family => family.value === "rock").albumCount, 1);
  assert.deepEqual(evaluate("genreAlbums", scope).map(album => album.id), ["B/album"]);
  assert.deepEqual(evaluate("genreAlbums", { ...scope, activeGenre: "other" }).map(album => album.id), ["A/unknown"]);
  assert.deepEqual(evaluate("filteredAlbums", scope), albums, "Albums remains unclassified and unfiltered by family");
});
