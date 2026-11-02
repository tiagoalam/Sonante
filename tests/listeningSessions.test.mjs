import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const read = file => readFileSync(new URL('../src/' + file, import.meta.url), 'utf8');
function compile(file, deps = {}) { const exports = {}; new Function('require', 'exports', ts.transpileModule(read(file), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports); return exports; }
const helpers = compile('utils/listeningSessions.ts');
const local = { kind: 'local' }, nav = { kind: 'navidrome', account_id: 'a'.repeat(64) };
const album = id => ({ id, title: 'Album ' + id, artist: 'Artist', year: 1970, genres: ['Jazz'], added_at: null });
const track = (id, duration = 600, locator = { kind: 'local', uri: 'Local/' + id + '.flac' }) => ({ title: 'Song', artist: 'Artist', album: 'Album', duration, media_locator: locator });
const scene = () => ({ id: 'session-1', name: 'Evening', created_at: 1234, target: 3600, source: local, albums: [helpers.sessionAlbum(album('a'), [track('a'), track('b')], local, 'discovery')], cursor: 0, completed: false });
const store = () => { const values = new Map(); return { values, getItem: k => values.get(k) ?? null, setItem: (k, v) => values.set(k, v) }; };
test('time targets preserve whole albums and order instead of cutting the final track', async () => {
  const result = await helpers.prepareSessionAlbums(['a', 'b', 'c'].map(album), local, 3600, 'discovery', async a => [track(a.id + '1', 1200), track(a.id + '2', 1200)], () => true);
  assert.deepEqual(result.albums.map(a => a.id), ['a', 'b']); assert.equal(result.albums.reduce((sum,a)=>sum+a.duration,0), 4800); assert.deepEqual(result.albums.map(a=>a.keys.length), [2,2]);
  const near = await helpers.prepareSessionAlbums(['a', 'b'].map(album), local, 3600, 'discovery', async a => [track(a.id, 2100)], () => true);
  assert.equal(near.albums.length, 2); assert.equal(near.albums.reduce((s, a) => s + a.duration, 0), 4200);
});
test('an album longer than the target stays complete, and unknown duration is never fabricated', async () => {
  for (const duration of [7200, undefined]) {
    const result = await helpers.prepareSessionAlbums(['a', 'b'].map(album), local, 1800, 'recent', async a => [{ ...track(a.id), duration }], () => true);
    assert.equal(result.albums.length, 1); assert.equal(result.albums[0].duration, duration ?? null);
  }
});
test('history filtering uses source identities, not matching artist/album text', async () => {
  const result = await helpers.prepareSessionAlbums(['a', 'b'].map(album), local, 1800, 'unplayed', async a => [track(a.id, 1800)], () => true, new Set([helpers.sessionTrackKey(track('a').media_locator)]));
  assert.deepEqual(result.albums.map(a => a.id), ['b']);
});
test('preparation is cancellable between reads and has bounded work on a large catalog', async () => {
  let live = true, calls = 0;
  await assert.rejects(helpers.prepareSessionAlbums(['a', 'b'].map(album), local, 3600, 'discovery', async a => { calls++; live = false; return [track(a.id)]; }, () => live), e => e === 'SESSION_CANCELLED'); assert.equal(calls, 1);
  calls = 0;
  await assert.rejects(helpers.prepareSessionAlbums(Array.from({ length: 4000 }, (_, i) => album(String(i))), local, 3600, 'unplayed', async () => { calls++; return [track('heard')]; }, () => true, new Set([helpers.sessionTrackKey(track('heard').media_locator)])), e => e === 'SESSION_EMPTY'); assert.equal(calls, 24);
});
test('complete source batches reject wrong accounts, foreign sources, duplicate references and empty discs', () => {
  for (const tracks of [[], [track('a', 60, { kind: 'radio', station_id: 'r' })], [track('a'), track('a')]]) assert.throws(() => helpers.sessionAlbum(album('a'), tracks, local, 'recent'));
  assert.throws(() => helpers.sessionAlbum(album('a'), [track('a', 60, { ...nav, account_id: 'b'.repeat(64), connection_id: 'ephemeral', song_id: 'a' })], nav, 'recent'));
});
test('Navidrome durable identities ignore ephemeral connection ids and distinguish accounts', () => {
  const key = connection_id => helpers.sessionTrackKey({ ...nav, connection_id, song_id: 'song-1' });
  assert.equal(key('first'), key('second')); assert.ok(!key('ephemeral').includes('ephemeral'));
  assert.notEqual(key('first'), helpers.sessionTrackKey({ ...nav, account_id: 'b'.repeat(64), connection_id: 'first', song_id: 'song-1' }));
});
test('recent selection sorts missing addition dates last without mutating the catalog', () => {
  const input = [album('unknown'), { ...album('older'), added_at: 20 }, { ...album('new'), added_at: 40 }];
  assert.deepEqual(helpers.orderedCandidates(input, 'recent', () => 0).map(a => a.id), ['new', 'older', 'unknown']); assert.equal(input[0].id, 'unknown');
  assert.equal(helpers.sessionAlbum(album('unknown'), [track('a')], local, 'recent').reason, 'discovery', 'missing addition dates never imply a recent addition');
});
test('fixed selections and resume cursor round-trip without authenticated URLs or artwork', () => {
  const memory = store(), value = scene(); value.cursor = 1;
  helpers.writeSessions(memory, [value]); assert.deepEqual(helpers.readSessions(memory), [value]);
  const raw = memory.getItem(helpers.SESSION_KEY); assert.ok(!raw.includes('thumb')); assert.ok(!raw.includes('connection_id')); assert.ok(!raw.includes('http'));
});
test('corrupt, oversized and invalid stores are preserved; saving fails explicitly at capacity', () => {
  const memory = store(); memory.setItem(helpers.SESSION_KEY, 'broken'); assert.throws(() => helpers.readSessions(memory)); assert.equal(memory.getItem(helpers.SESSION_KEY), 'broken');
  for (const value of [{ ...scene(), cursor: 2 }, { ...scene(), target: 1 }, { ...scene(), albums: [] }]) assert.equal(helpers.validSession(value), false);
  assert.throws(() => helpers.writeSessions(memory, Array.from({ length: 25 }, (_, i) => ({ ...scene(), id: String(i) })))); assert.equal(memory.getItem(helpers.SESSION_KEY), 'broken');
});
test('dormant progress tracking performs no queue or storage reads', async () => {
  let reads = 0;
  const progress = compile('utils/listeningSessionProgress.ts', { './listeningSessions': helpers, '../services/audio': { audioService: { getQueueEditSnapshot: async () => { reads++; } } }, './listeningSessionStore': { listeningSessionStore: { list: () => { reads++; } } } });
  await progress.observeSessionProgress({ state: 'play', current_media: { kind: 'local', uri: 'Local/a.flac', queue_index: 0 } }); assert.equal(reads, 0);
});
test('progress is bound to the committed queue revision, survives leaving the view, and cannot follow another queue', async () => {
  let saved = scene(), revision = 'q1', reads = 0;
  const tracks = [track('a'), track('b')];
  const progress = compile('utils/listeningSessionProgress.ts', { './listeningSessions': helpers, '../services/audio': { audioService: { getQueueEditSnapshot: async () => { reads++; return { revision, tracks }; } } }, './listeningSessionStore': { listeningSessionStore: { list: () => [saved], updateProgress: value => { saved = value; } } } });
  await progress.bindSessionProgress(saved, () => true);
  const status = index => ({ state: 'play', snapshot_consistent: true, current_media: { kind: 'local', uri: tracks[index].media_locator.uri, queue_index: index } });
  await progress.observeSessionProgress(status(1)); assert.equal(saved.cursor, 1);
  const before = reads; await progress.observeSessionProgress(status(1)); assert.equal(reads, before, 'no per-second queue polling');
  revision = 'different-queue'; await progress.observeSessionProgress(status(0)); assert.equal(saved.cursor, 1);
});
test('late bind from a superseded play cannot capture a newer queue', async () => {
  let release, calls = 0;
  const progress = compile('utils/listeningSessionProgress.ts', { './listeningSessions': helpers, '../services/audio': { audioService: { getQueueEditSnapshot: async () => { calls++; await new Promise(r => { release = r; }); return { revision: 'q', tracks: [track('a'), track('b')] }; } } }, './listeningSessionStore': { listeningSessionStore: { updateProgress: () => { throw Error('not bound'); } } } });
  const binding = progress.bindSessionProgress(scene(), () => true); progress.suspendSessionProgress(); release(); await binding;
  await progress.observeSessionProgress({ state: 'play', current_media: { kind: 'local', uri: 'Local/a.flac', queue_index: 0 } }); assert.equal(calls, 1);
});
test('a delayed pre-Play status does not detach the correctly bound new queue', async () => {
  let saved = scene();
  const progress = compile('utils/listeningSessionProgress.ts', { './listeningSessions': helpers, '../services/audio': { audioService: { getQueueEditSnapshot: async () => ({ revision: 'q1', tracks: [track('a'), track('b')] }) } }, './listeningSessionStore': { listeningSessionStore: { list: () => [saved], updateProgress: value => { saved = value; } } } });
  await progress.bindSessionProgress(saved, () => true);
  await progress.observeSessionProgress({ state: 'play', current_media: { kind: 'local', uri: 'Local/old.flac', queue_index: 0 } });
  await progress.observeSessionProgress({ state: 'play', current_media: { kind: 'local', uri: 'Local/b.flac', queue_index: 1 } });
  assert.equal(saved.cursor, 1);
});
test('temporary queue guards preserve the binding with a bounded retry and no audio command', async () => {
  let saved = scene(), blocked = false, calls = 0;
  const progress = compile('utils/listeningSessionProgress.ts', { './listeningSessions': helpers, '../services/audio': { audioService: { getQueueEditSnapshot: async () => { calls++; if (blocked) throw 'QUEUE_BUSY'; return { revision: 'q1', tracks: [track('a'), track('b')] }; } } }, './listeningSessionStore': { listeningSessionStore: { list: () => [saved], updateProgress: value => { saved = value; } } } });
  await progress.bindSessionProgress(saved, () => true); blocked = true;
  const status = { state: 'play', current_media: { kind: 'local', uri: 'Local/b.flac', queue_index: 1 } };
  await progress.observeSessionProgress(status, 10); assert.equal(saved.cursor, 0);
  blocked = false; await progress.observeSessionProgress(status, 20); assert.equal(calls, 2);
  await progress.observeSessionProgress(status, 5010); assert.equal(saved.cursor, 1);
});
function sourceFixture(options = {}) {
  const calls = [];
  const queue = compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue();
  const navTracks = compile('utils/navidromeTracks.ts');
  const service = compile('services/listeningSessions.ts', {
    '../utils/listeningSessions': helpers, '../utils/localSourceReadQueue': { localSourceReadQueue: queue }, '../utils/navidromeTracks': navTracks,
    './smartPlaylists': { smartPlaylistsService: { sources: async () => { calls.push('source'); return options.sources?.() ?? { local: true, plex_server_id: 'server-a', navidrome_account_id: nav.account_id }; }, catalog: async () => [album('a')] } },
    './audio': { audioService: { getLocalAlbums: async () => { calls.push('catalog'); return [ { ...album('a'), folder_path: 'Local/A', source_id: 'origin-a', source_label: 'Jazz', added_at: '2026-11-01T00:00:00Z' } ]; }, getLocalAlbumTracks: async () => { calls.push('local'); return [{ disc: { number: 2 }, tracks: [{ item_type: 'file', path: 'Local/2.flac', name: 'Second' }] }, { disc: { number: 1 }, tracks: [{ item_type: 'file', path: 'Local/1.flac', name: 'First' }] }]; } } },
    './plex': { plexService: { getAlbumTracks: async () => [{ title: 'Plex song', duration_ms: 123456, media_locator: { kind: 'plex', server_id: options.plexServer ?? 'server-a', part_key: '/library/parts/1/file.flac' } }] } },
    './navidrome': { navidromeService: { status: async () => { calls.push('nav-status'); return options.navStatus?.() ?? { connection_id: 'ephemeral', account_id: nav.account_id }; }, album: async () => { calls.push('nav-album'); return { songs: [{ id: 'two', title: 'Two', artist: 'Artist', album: 'A', duration: 80, disc: 2, track: 1 }, { id: 'one', title: 'One', artist: 'Artist', album: 'A', duration: 90, disc: 1, track: 1 }] }; } } },
  }); return { service, queue, calls };
}
test('Local catalog keeps folder scope and source reads retain disc order', async () => {
  const f = sourceFixture(), catalog = await f.service.sessionCatalog(local, () => true);
  assert.equal(catalog[0].scope_id, 'origin-a'); assert.equal(catalog[0].scope_label, 'Jazz');
  const tracks = await f.service.sessionAlbumTracks(local, album('a'), () => true);
  assert.deepEqual(tracks.map(t => t.media_locator.uri), ['Local/1.flac', 'Local/2.flac']);
});
test('disabled sources never read the catalog or album, and stale queued reads never start', async () => {
  const disabled = sourceFixture({ sources: () => ({ local: false }) });
  await assert.rejects(disabled.service.sessionCatalog(local, () => true), e => e === 'SESSION_SOURCE'); assert.deepEqual(disabled.calls, ['source']);
  const f = sourceFixture(); let release, live = true;
  const occupied = f.queue(() => new Promise(resolve => { release = resolve; })); await new Promise(r => setImmediate(r));
  const read = f.service.sessionAlbumTracks(local, album('a'), () => live); const refused = assert.rejects(read, e => e === 'SESSION_CANCELLED');
  await new Promise(r => setImmediate(r)); live = false; release(); await occupied; await refused; assert.ok(!f.calls.includes('local'));
});
test('Plex session tracks preserve durations and refuse a different server', async () => {
  const source = { kind: 'plex', server_id: 'server-a', library_id: '7' };
  assert.equal((await sourceFixture().service.sessionAlbumTracks(source, album('a'), () => true))[0].duration, 123.456);
  await assert.rejects(sourceFixture({ plexServer: 'wrong-server' }).service.sessionAlbumTracks(source, album('a'), () => true), e => e === 'SESSION_READ');
});
test('Navidrome session batches are ordered and a connection/account change refuses the entire batch', async () => {
  const tracks = await sourceFixture().service.sessionAlbumTracks(nav, album('a'), () => true);
  assert.deepEqual(tracks.map(t => t.media_locator.song_id), ['one', 'two']); assert.ok(tracks.every(t => t.media_locator.account_id === nav.account_id));
  let status = 0;
  const f = sourceFixture({ navStatus: () => ({ connection_id: ++status === 1 ? 'first' : 'second', account_id: nav.account_id }) });
  await assert.rejects(f.service.sessionAlbumTracks(nav, album('a'), () => true), e => e === 'SESSION_SOURCE');
});


test('30, 60 and 90 minute targets find distinct complete-album combinations rather than stopping at the first oversized alternative',async()=>{
 for(const minutes of [30,60,90]){
  const candidates=[40,45,20,10,30].map((value,index)=>({...album(String(index)),seconds:value*60}));let reads=0;
  const result=await helpers.prepareSessionAlbums(candidates,local,minutes*60,'discovery',async a=>{reads++;return[track(a.id,a.seconds)]},()=>true);
  assert.equal(result.albums.reduce((sum,a)=>sum+a.duration,0),minutes*60);
  assert.deepEqual(result.albums.map(a=>a.id),[...result.albums.map(a=>a.id)].sort());
  assert.deepEqual(result.tracks.flat().map(t=>helpers.sessionTrackKey(t.media_locator)),result.albums.flatMap(a=>a.keys));
  assert.ok(reads<=5);
 }
});
test('a later single album may replace a poor first fit; a close enough fit finishes without enumerating the collection',async()=>{
 let reads=0;const result=await helpers.prepareSessionAlbums([42,80,60,10].map((m,i)=>({...album(String(i)),seconds:m*60})),local,3600,'recent',async a=>{reads++;return[track(a.id,a.seconds)]},()=>true);
 assert.equal(result.albums.length,1);assert.equal(result.albums[0].duration,3600);assert.equal(reads,3);
 reads=0;const near=await helpers.prepareSessionAlbums([59,40,20].map((m,i)=>({...album(String(i)),seconds:m*60})),local,3600,'discovery',async a=>{reads++;return[track(a.id,a.seconds)]},()=>true);assert.equal(near.albums[0].duration,3540);assert.equal(reads,1);
});
test('duration search stops after eight eligible alternatives, still caps the result at six albums and 1500 tracks, and keeps unknown duration honest',async()=>{
 let reads=0;const result=await helpers.prepareSessionAlbums(Array.from({length:1000},(_,i)=>album(String(i))),local,5400,'discovery',async a=>{reads++;return Array.from({length:500},(_,i)=>track(a.id+'-'+i,1))},()=>true);
 assert.equal(reads,8);assert.ok(result.albums.length<=6);assert.equal(result.tracks.flat().length,1500);
 reads=0;const unknown=await helpers.prepareSessionAlbums([album('known'),album('unknown'),album('fit')],local,3600,'discovery',async a=>{reads++;return a.id==='unknown'?[{...track(a.id),duration:undefined}]:[track(a.id,a.id==='fit'?3600:2400)]},()=>true);assert.equal(unknown.albums.length,1);assert.equal(unknown.albums[0].id,'fit');assert.equal(reads,3);
});
test('an unknown duration or a foreign source cannot be used as a fabricated time match',async()=>{
 const unknown=await helpers.prepareSessionAlbums([album('unknown'),album('later')],local,3600,'discovery',async a=>[{...track(a.id),duration:undefined}],()=>true);assert.equal(unknown.albums[0].duration,null);
 await assert.rejects(helpers.prepareSessionAlbums([album('foreign')],local,3600,'discovery',async()=>[track('wrong',3600,{kind:'plex',server_id:'server',part_key:'part'})],()=>true),e=>e==='SESSION_READ');
});
