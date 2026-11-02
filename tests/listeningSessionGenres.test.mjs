import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import * as families from '../src/utils/localGenreFamilies.ts';
function compile(file, deps = {}) {
  const exports = {}; const source = readFileSync(new URL('../src/' + file, import.meta.url), 'utf8');
  new Function('require', 'exports', ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports);
  return exports;
}
const smart = compile('utils/smartPlaylists.ts', { './localGenreFamilies': families });
const genres = compile('utils/listeningSessionGenres.ts', { './localGenreFamilies': families, './smartPlaylists': smart });
const nav = { kind: 'navidrome', account_id: 'a'.repeat(64) };
const album = (...tags) => ({ id: 'album', title: 'Album', artist: 'Artist', year: null, added_at: null, genres: tags });
test('Navidrome session genres consolidate aliases, subgenres and compound tags using the existing Local families', () => {
  const albums = [album('Hard Bop'), album('Smooth Jazz'), album('Jazz / Rock'), album('Brazilian Jazz | Fusion'), album('Bossa Nova'), album('British Reggae')];
  assert.deepEqual(genres.sessionGenreValues(albums, nav), ['brazilian', 'jazz', 'reggae']);
  for (const value of albums.slice(0, 3)) assert.ok(genres.sessionMatchesGenre(value, nav, 'jazz'));
  assert.ok(!genres.sessionMatchesGenre(albums[3], nav, 'jazz')); assert.ok(genres.sessionMatchesGenre(albums[3], nav, 'brazilian'));
  assert.deepEqual(genres.sessionGenreValues(albums, { kind: 'local' }), genres.sessionGenreValues(albums, nav));
});
test('unknown and numeric genres remain Others, without guessed mappings or changes to raw tags', () => {
  const albums = [album('16'), album('Uncatalogued style'), album('Jazz / Unknown')];
  const before = JSON.stringify(albums);
  assert.deepEqual(genres.sessionGenreValues(albums, nav), ['other']);
  assert.ok(albums.every(a => genres.sessionMatchesGenre(a, nav, 'other')));
  assert.ok(albums.every(a => genres.sessionMatchesGenre(a, nav, '')));
  assert.equal(JSON.stringify(albums), before);
});
test('Plex sessions retain their exact server genres and matching behavior', () => {
  const plex = { kind: 'plex', server_id: 's', library_id: '1' }, albums = [album('Hard Bop'), album('Jazz / Rock'), album('Jazz')];
  assert.deepEqual(genres.sessionGenreValues(albums, plex), ['Hard Bop', 'Jazz', 'Jazz / Rock']);
  assert.ok(genres.sessionMatchesGenre(albums[0], plex, 'Hard Bop'));
  assert.ok(!genres.sessionMatchesGenre(albums[0], plex, 'jazz'));
  assert.ok(!genres.sessionMatchesGenre(albums[1], plex, 'Jazz'));
});
