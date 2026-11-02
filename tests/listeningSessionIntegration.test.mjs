import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const read = file => readFileSync(new URL('../src/' + file, import.meta.url), 'utf8');
function compile(file, deps = {}) {
  const exports = {};
  new Function('require', 'exports', ts.transpileModule(read(file), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports);
  return exports;
}
const flush = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const source = { kind: 'local' };
const album = { id: 'album-1', title: 'Album', artist: 'Artist', year: '1970', genres: ['Jazz'], source_id: 'source-1', source_label: 'Jazz', added_at: null };
const candidate = { ...album, year: 1970 };
function fixture(options = {}) {
  const calls = [];
  const queue = compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue();
  const { audioService } = compile('services/audio.ts', {
    './config': { ensureDirectOutputChoice: async () => {} },
    '../utils/localSourceReadQueue': { localSourceReadQueue: queue },
    '../utils/localCoverBatch': compile('utils/localCoverBatch.ts'),
    '../utils/playbackIntent': compile('utils/playbackIntent.ts'),
    '../utils/frontendDiagnostics': { diagnosticInvoke: (invoke, command, args) => invoke(command, args) },
    '@tauri-apps/api/core': { invoke: async command => {
      calls.push(command);
      if (command === 'get_local_albums') return options.catalog ? options.catalog() : [album];
      if (command === 'get_local_album_tracks') return [{ disc: { number: 1 }, tracks: [{ item_type: 'file', name: 'Track', path: 'Local/album-1/1.flac', duration: 120 }] }];
      if (command === 'play_tracks') return;
      throw new Error('Unexpected command: ' + command);
    } },
  });
  const service = compile('services/listeningSessions.ts', {
    './audio': { audioService },
    './smartPlaylists': { smartPlaylistsService: { sources: async () => ({ local: true, plex_server_id: null }) } },
    './plex': { plexService: {} }, './navidrome': { navidromeService: {} },
    '../utils/localSourceReadQueue': { localSourceReadQueue: queue },
    '../utils/navidromeTracks': compile('utils/navidromeTracks.ts'),
    '../utils/listeningSessions': compile('utils/listeningSessions.ts'),
  });
  return { service, audioService, calls };
}
const intent = () => ({ token: Promise.resolve('synthetic-intent'), current: () => true });
test('Local session catalog reaches IPC and completes through the actual queued audio service', async () => {
  const f = fixture(); const result = f.service.sessionCatalog(source, () => true);
  await flush();
  assert.deepEqual(f.calls, ['get_local_albums'], 'a read queued inside itself never reaches native IPC');
  const albums = await result; assert.equal(albums.length, 1); assert.equal(albums[0].scope_id, 'source-1');
});
test('catalog serialization still gives a waiting playback command priority over the next album read', async () => {
  const pending = deferred(), f = fixture({ catalog: () => pending.promise });
  const catalog = f.service.sessionCatalog(source, () => true); await flush();
  assert.deepEqual(f.calls, ['get_local_albums']);
  const play = f.audioService.playTracks([], 0, intent());
  const tracks = f.service.sessionAlbumTracks(source, candidate, () => true);
  await flush(); assert.deepEqual(f.calls, ['get_local_albums']);
  pending.resolve([album]); await Promise.all([catalog, play, tracks]);
  assert.deepEqual(f.calls, ['get_local_albums', 'play_tracks', 'get_local_album_tracks']);
});
test('a cancelled catalog cannot publish late data and the replacement read completes', async () => {
  const pending = deferred(); let number = 0, live = true;
  const f = fixture({ catalog: () => ++number === 1 ? pending.promise : [{ ...album, id: 'new-album' }] });
  const old = f.service.sessionCatalog(source, () => live);
  const rejected = assert.rejects(old, e => e === 'SESSION_CANCELLED'); await flush();
  live = false; const replacement = f.service.sessionCatalog(source, () => true);
  pending.resolve([album]); await rejected;
  assert.equal((await replacement)[0].id, 'new-album'); assert.equal(f.calls.filter(c => c === 'get_local_albums').length, 2);
});
test('native refusal releases the real queue without automatic retry or playback side effects', async () => {
  let number = 0;
  const f = fixture({ catalog: () => { if (++number === 1) throw 'CATALOG_UNAVAILABLE'; return [album]; } });
  await assert.rejects(f.service.sessionCatalog(source, () => true), e => e === 'CATALOG_UNAVAILABLE');
  assert.deepEqual(f.calls, ['get_local_albums']);
  assert.equal((await f.service.sessionCatalog(source, () => true)).length, 1);
  assert.deepEqual(f.calls, ['get_local_albums', 'get_local_albums']);
});
