import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
function compile(file, deps = {}) {
  const exports = {}, source = readFileSync(new URL('../src/' + file, import.meta.url), 'utf8');
  new Function('require', 'exports', ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports);
  return exports;
}
const sessions = compile('utils/listeningSessions.ts');
const polish = compile('utils/listeningSessionPolish.ts', { './listeningSessions': sessions });
const local = { kind: 'local' };
const candidate = (id, added_at = null) => ({ id, title: 'Album ' + id, artist: 'Artist', year: 1970, genres: ['Jazz'], added_at });
const tracks = (id, count) => Array.from({ length: count }, (_, index) => ({ title: 'Track ' + index, artist: 'Artist', album: 'Album ' + id, duration: 90, media_locator: { kind: 'local', uri: `Local/${id}/${index}.flac` } }));
const album = (id, count) => sessions.sessionAlbum(candidate(id), tracks(id, count), local, 'discovery');
const scene = () => ({ id: 'session', name: 'Evening', created_at: 1000, source: local, target: 3600, albums: [album('a', 2), album('b', 3), album('c', 1)], cursor: 3, started_at: 1000, completed: false });
test('visible album/track positions handle the first track and every album boundary', () => {
  const value = scene(), before = JSON.stringify(value);
  for (const [cursor, album, track, count] of [[0, 1, 1, 2], [1, 1, 2, 2], [2, 2, 1, 3], [3, 2, 2, 3], [4, 2, 3, 3], [5, 3, 1, 1]]) {
    assert.deepEqual(polish.sessionPosition({ ...value, cursor }), { albumIndex: album - 1, album, albums: 3, track, tracks: count, position: cursor + 1, total: 6 });
  }
  assert.equal(JSON.stringify(value), before, 'presentation cannot alter stored progress');
});
test('missing or invalid positions are never presented as a valid listening position', () => {
  assert.equal(polish.sessionPosition(null), null); assert.equal(polish.sessionPosition({ albums: [], cursor: 0 }), null);
  for (const cursor of [-1, 6, NaN, 1.5]) assert.equal(polish.sessionPosition({ ...scene(), cursor }), null);
});
test('cover alternatives exclude every selected album and duplicate ids without mutating the filtered pool', () => {
  const pool = [candidate('a'), candidate('b'), candidate('c'), candidate('d'), candidate('d'), candidate('e')], selected = scene();
  const before = JSON.stringify([pool, selected]);
  assert.deepEqual(new Set(polish.sessionReplacementCandidates(pool, selected, 'discovery', () => 0).map(a => a.id)), new Set(['d', 'e']));
  assert.equal(JSON.stringify([pool, selected]), before);
  assert.deepEqual(polish.sessionReplacementCandidates(pool.slice(0, 3), selected, 'recent'), []);
});
test('recent alternatives retain the requested priority and support bounded pages of covers', () => {
  const pool = [candidate('d', 10), candidate('e', 40), candidate('f', 20), candidate('g', 30)];
  const values = polish.sessionReplacementCandidates(pool, scene(), 'recent', () => 0);
  assert.deepEqual(values.map(a => a.id), ['e', 'g', 'f', 'd']);
  assert.equal(values.slice(0, 3).length, 3); assert.equal(values.slice(3, 6).length, 1);
});
test('only the explicitly chosen album is read, keeping complete tracks and all other albums unchanged', async () => {
  const value = scene(), before = JSON.stringify(value), calls = [];
  const result = await polish.prepareSessionReplacement(value, 'b', candidate('chosen'), async selected => { calls.push(selected.id); return tracks(selected.id, 4); }, () => true);
  assert.deepEqual(calls, ['chosen']); assert.deepEqual(result.map(a => a.id), ['a', 'chosen', 'c']);
  assert.equal(result[1].keys.length, 4); assert.equal(result[1].reason, 'selected');
  assert.equal(result[0], value.albums[0]); assert.equal(result[2], value.albums[2]); assert.equal(JSON.stringify(value), before);
});
test('closing or changing source context during the chosen read cannot apply a late replacement', async () => {
  const value = scene(), before = JSON.stringify(value); let live = true, reads = 0;
  await assert.rejects(polish.prepareSessionReplacement(value, 'b', candidate('d'), async () => { reads++; live = false; return tracks('d', 1); }, () => live), e => e === 'SESSION_CANCELLED');
  assert.equal(reads, 1); assert.equal(JSON.stringify(value), before);
});
test('a missing slot, existing album or foreign source is refused without partial selection changes', async () => {
  const value = scene(), before = JSON.stringify(value); let reads = 0;
  const read = async () => { reads++; return tracks('d', 1); };
  await assert.rejects(polish.prepareSessionReplacement(value, 'gone', candidate('d'), read, () => true), e => e === 'SESSION_CHANGED');
  await assert.rejects(polish.prepareSessionReplacement(value, 'a', candidate('b'), read, () => true), e => e === 'SESSION_CHANGED');
  assert.equal(reads, 0);
  await assert.rejects(polish.prepareSessionReplacement(value, 'b', candidate('d'), async () => [{ title: 'Foreign', artist: 'Artist', album: 'Album', media_locator: { kind: 'plex', server_id: 's', part_key: '/library/parts/1/file.flac' } }], () => true), e => e === 'SESSION_READ');
  assert.equal(JSON.stringify(value), before);
});
test('the session size limit refuses a replacement without cutting its album', async () => {
  const value = { ...scene(), albums: [album('a', 500), album('b', 500), album('c', 400), album('d', 1)] };
  const before = JSON.stringify(value);
  await assert.rejects(polish.prepareSessionReplacement(value, 'd', candidate('e'), async () => tracks('e', 500), () => true), e => e === 'SESSION_READ');
  assert.equal(JSON.stringify(value), before);
});
test('manual replacement explanations persist alongside older sessions without claiming recent or unheard status', async () => {
  const old = scene();
  const albums = await polish.prepareSessionReplacement(old, 'b', candidate('d'), async () => tracks('d', 2), () => true);
  const edited = { ...old, id: 'edited', albums, cursor: 0, started_at: undefined };
  const memory = new Map(), storage = { getItem: key => memory.get(key) ?? null, setItem: (key, value) => memory.set(key, value) };
  sessions.writeSessions(storage, [old, edited]); const stored = sessions.readSessions(storage);
  assert.deepEqual(stored[0], old); assert.equal(stored[1].albums[1].reason, 'selected'); assert.equal(stored[1].cursor, 0);
});
