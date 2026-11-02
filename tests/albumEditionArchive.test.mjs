import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const compile = (file, deps = {}, env = {}) => {
  const exports = {};
  new Function('require', 'exports', ...Object.keys(env), ts.transpileModule(readFileSync(new URL('../src/' + file, import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports, ...Object.values(env)); return exports;
};
const sizes = compile('utils/albumEditionSizes.ts'), editions = compile('utils/albumEditions.ts');
const analysis = compile('utils/albumEditionAnalysis.ts', { './albumEditionSizes': sizes, './albumEditions': editions });
const archiveModule = compile('utils/albumEditionArchive.ts', { './albumEditionAnalysis': analysis });
const profile = n => ({ sizes: [123, n], titles: ['Track A', 'Track B'], cover_id: null });
const entry = (source, id = 'one', title = 'Album') => ({ key: JSON.stringify([source, source === 'navidrome' ? 'account' : 'server', id]), source, origin: 'Library', album: { id, rating_key: id, title, artist: 'Artist', track_count: 2, song_count: 2 }, serverId: 'server', libraryId: '7', accountId: 'account', connectionId: 'ephemeral' });
const namespace = 'a'.repeat(64), otherNamespace = 'b'.repeat(64);
function storage() {
  const data = new Map(), writes = []; let fail = false;
  return { data, writes, set fail(value) { fail = value; },
    async get(keys) { if (fail) throw 'UNAVAILABLE'; return new Map(keys.filter(key => data.has(key)).map(key => [key, structuredClone(data.get(key))])); },
    async put(record) { if (fail) throw 'QUOTA'; data.set(record.key, structuredClone(record)); writes.push(record.key); },
    async clear(ns, keys) { if (fail) throw 'UNAVAILABLE'; for (const key of data.keys()) if (key.startsWith(ns + ':') && (!keys || keys.includes(key))) data.delete(key); },
  };
}
function service(disk, options = {}) {
  const calls = [], archive = archiveModule.createEditionArchive(disk, archiveModule.editionArchiveDigest, options.now ?? Date.now);
  const deps = {
    '../utils/albumEditionArchive': { ...archiveModule, createEditionArchive: () => archive, createIndexedEditionStorage: () => disk },
    '../utils/albumEditionAnalysis': analysis, '../utils/albumEditionSizes': sizes,
    '../utils/localAlbumCatalogSession': { localAlbumCatalog: { getSnapshot: () => { throw 'NO_CATALOG_EXPECTED'; } } },
    '../utils/localSourceReadQueue': { localSourceReadQueue: compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue() },
    '../utils/musicSources': { sourceEnabled: (config, source) => config.sources[source] },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'get_edition_cache_namespace') return options.namespace ?? namespace;
      assert.equal(command, 'get_edition_profile'); calls.push(args.source);
      return options.read ? options.read(args.source) : profile(args.source.album_id.includes('deluxe') ? 457 : 456);
    } }, './audio': {}, './plex': {}, './navidrome': {}, './smartPlaylists': {},
  };
  return { calls, ...compile('services/albumEditions.ts', deps) };
}
const config = { sources: { local: true, plex: true, navidrome: true }, local_folders: ['/synthetic/music'], plex_token: 'synthetic-secret', plex_server_id: 'server' };
const restoredService = async (disk, entries, options = {}) => {
  const f = service(disk, options), scope = f.editionReadScope(options.config ?? config, {}, 0, 0);
  const restored = await f.restoreEditionProfiles(entries, scope, () => true); return { ...f, scope, restored };
};
test('successful observations survive a completely new service and renderer, including negative copy comparisons', async () => {
  const disk = storage(), entries = [entry('local'), entry('plex'), entry('navidrome'), entry('local', 'deluxe', 'Album (Deluxe Edition)')];
  const first = await restoredService(disk, entries); assert.equal(first.calls.length, 0);
  for (const e of entries) await first.readEditionProfile(e, first.scope, () => true);
  assert.equal(first.calls.length, 4); assert.equal(disk.writes.length, 4);
  const second = await restoredService(disk, entries, { read: () => assert.fail('a restart must not repeat successful size reads') });
  assert.equal(second.restored.size, 4);
  for (const e of entries) assert.deepEqual(await second.readEditionProfile(e, second.scope, () => true), first.cachedEditionProfiles([e], first.scope).get(e.key));
  const group = editions.groupAlbumEditions(entries)[0]; assert.equal(analysis.analyzeEditionGroup(group, second.restored).variants.length, 2);
  assert.equal(analysis.verifiedEditionGroup({ ...group, entries: entries.slice(0, 3) }, second.restored), null);
  assert.deepEqual(second.calls, []);
});
test('interruption saves each completed observation; restart reads only incomplete or new entries', async () => {
  const disk = storage(), entries = [entry('local'), entry('plex'), entry('navidrome')];
  const first = await restoredService(disk, entries); await first.readEditionProfile(entries[0], first.scope, () => true);
  let live = true;
  const cancelled = await restoredService(disk, entries, { read: () => { live = false; return profile(456); } });
  await assert.rejects(cancelled.readEditionProfile(entries[1], cancelled.scope, () => live)); assert.equal(disk.data.size, 1);
  const next = await restoredService(disk, entries); assert.equal(next.restored.size, 1);
  for (const e of entries) await next.readEditionProfile(e, next.scope, () => true);
  assert.deepEqual(next.calls.map(c => c.kind), ['plex', 'navidrome']);
});
test('a checkpoint is committed before a successful observation is published', async () => {
  const disk = storage(), originalPut = disk.put; let release, committed = false;
  const wait = new Promise(resolve => { release = resolve; });
  disk.put = async value => { await wait; await originalPut(value); committed = true; };
  const entries = [entry('local')], f = await restoredService(disk, entries); let published = false;
  const read = f.readEditionProfile(entries[0], f.scope, () => true).then(value => { assert.equal(committed, true); published = true; return value; });
  for (let i = 0; i < 20; i++) await Promise.resolve(); assert.equal(published, false); assert.equal(disk.data.size, 0);
  release(); await read; assert.equal(disk.data.size, 1);
});
test('a changed entry cannot borrow a prepared fingerprint belonging to its old metadata', async () => {
  const disk = storage(), entries = [entry('local')], f = await restoredService(disk, entries);
  await f.readEditionProfile(entries[0], f.scope, () => true);
  await f.readEditionProfile({ ...entries[0], album: { ...entries[0].album, track_count: 8 } }, f.scope, () => true);
  assert.equal(f.calls.length, 2); assert.equal(disk.data.size, 1);
});
test('catalog changes invalidate only affected album fingerprints; new Navidrome connections reuse the same account observations', async () => {
  const disk = storage(), entries = ['local', 'plex', 'navidrome'].map(source => entry(source));
  const first = await restoredService(disk, entries); for (const e of entries) await first.readEditionProfile(e, first.scope, () => true);
  const changed = entries.map(e => e.source === 'local' ? { ...e, album: { ...e.album, track_count: 3 } } : e.source === 'navidrome' ? { ...e, connectionId: 'renewed' } : e);
  const next = await restoredService(disk, changed); assert.deepEqual([...next.restored.keys()], [entries[1].key, entries[2].key]);
  for (const e of changed) await next.readEditionProfile(e, next.scope, () => true); assert.deepEqual(next.calls.map(c => c.kind), ['local']);
  const newAccount = { ...entries[2], key: JSON.stringify(['navidrome', 'different-account', 'one']), accountId: 'different-account' };
  const other = await restoredService(disk, [newAccount]); assert.equal(other.restored.size, 0);
});
test('configured Local roots, Plex credentials and native profile namespaces cannot relabel saved observations', async () => {
  const disk = storage(), entries = ['local', 'plex', 'navidrome'].map(source => entry(source));
  const first = await restoredService(disk, entries); for (const e of entries) await first.readEditionProfile(e, first.scope, () => true);
  const roots = await restoredService(disk, entries, { config: { ...config, local_folders: ['/different'] } }); assert.deepEqual([...roots.restored.keys()], entries.slice(1).map(e => e.key));
  const auth = await restoredService(disk, entries, { config: { ...config, plex_token: 'different-secret' } }); assert.deepEqual([...auth.restored.keys()], [entries[0].key, entries[2].key]);
  const other = await restoredService(disk, entries, { namespace: otherNamespace }); assert.equal(other.restored.size, 0);
  assert.ok(!JSON.stringify([...disk.data]).includes('synthetic-secret')); assert.ok(!JSON.stringify([...disk.data]).includes('/synthetic/music'));
});
test('explicit reanalysis discards this profile checkpoint durably; normal refresh restores it without file queries', async () => {
  const disk = storage(), entries = [entry('local'), entry('local', 'deluxe')];
  const f = await restoredService(disk, entries); for (const e of entries) await f.readEditionProfile(e, f.scope, () => true);
  assert.equal((await f.restoreEditionProfiles(entries, f.scope, () => true)).size, 2); assert.equal(f.calls.length, 2);
  f.invalidateEditionProfiles(f.scope); assert.equal((await f.restoreEditionProfiles(entries, f.scope, () => true)).size, 0); assert.equal(disk.data.size, 0);
  await f.readEditionProfile(entries[0], f.scope, () => true);
  const reopened = await restoredService(disk, entries); assert.equal(reopened.restored.size, 1);
});
test('unavailable files and persistence failures never become saved equality; usable results remain available this session', async () => {
  const disk = storage(), entries = [entry('local')];
  const missing = await restoredService(disk, entries, { read: () => ({ sizes: null, titles: ['A'], cover_id: null }) }); await missing.readEditionProfile(entries[0], missing.scope, () => true); assert.equal(disk.data.size, 0);
  const first = await restoredService(disk, entries); disk.fail = true;
  assert.deepEqual(await first.readEditionProfile(entries[0], first.scope, () => true), profile(456)); assert.equal(first.editionArchiveAvailable(first.scope), false); assert.equal(disk.data.size, 0);
  assert.deepEqual(await first.readEditionProfile(entries[0], first.scope, () => true), profile(456)); assert.equal(first.calls.length, 1);
  const unavailable = await restoredService(disk, entries); assert.equal(unavailable.editionArchiveAvailable(unavailable.scope), false);
  assert.deepEqual(await unavailable.readEditionProfile(entries[0], unavailable.scope, () => true), profile(456));
});
test('expired, future, corrupt, oversized and partial records are ignored; values are projected and copied', async () => {
  let time = 1000; const disk = storage(), archive = archiveModule.createEditionArchive(disk, archiveModule.editionArchiveDigest, () => time);
  const key = [...(await archive.keys(namespace, [entry('local')], () => null)).values()][0]; await archive.save(key, profile(456));
  const good = structuredClone(disk.data.get(key));
  for (const raw of [{ ...good, at: time + 1 }, { ...good, key: '../other' }, { ...good, profile: { ...good.profile, sizes: [0] } }, { ...good, profile: { ...good.profile, titles: Array(500).fill('a'.repeat(1024)), sizes: Array(500).fill(123) } }]) {
    disk.data.set(key, raw); assert.equal((await archive.restore([key])).size, 0);
  }
  disk.data.set(key, good); let result = await archive.restore([key]); result.get(key).sizes[0] = 999; assert.equal((await archive.restore([key])).get(key).sizes[0], 123);
  time += 30 * 86400000; assert.equal((await archive.restore([key])).size, 0);
  await assert.rejects(archive.restore(['../other'])); await assert.rejects(archive.keys('../other', [], () => null));
});

// Small asynchronous IndexedDB model: independent transaction snapshots commit
// only on completion; aborted writes are discarded. No WebView/profile access.
function indexedFixture() {
  const state = { profiles: new Map(), usage: new Map([['total', { count: 0, bytes: 0 }]]) }, events = []; let abortWrites = false;
  const db = { close() {}, transaction(_names, mode) {
    const tables = structuredClone(state); let pending = 0, ended = false;
    const tx = { objectStore: name => store(name), abort() { if (ended) return; ended = true; queueMicrotask(() => tx.onabort?.()); } };
    const check = () => queueMicrotask(() => { if (!ended && !pending) { if (mode === 'readwrite') { if (abortWrites) { tx.abort(); return; } state.profiles = tables.profiles; state.usage = tables.usage; } ended = true; events.push(mode); tx.oncomplete?.(); } });
    const request = fn => { const req = {}; pending++; queueMicrotask(() => { if (!ended) { req.result = fn(); req.onsuccess?.(); } pending--; check(); }); return req; };
    const cursor = sorted => {
      let index = 0; const req = {};
      const step = () => { pending++; queueMicrotask(() => { if (!ended) { const key = sorted[index++]; req.result = key === undefined ? null : { key, value: structuredClone(tables.profiles.get(key)), delete: () => tables.profiles.delete(key), continue: step }; req.onsuccess?.(); } pending--; check(); }); }; step(); return req;
    };
    const store = name => ({
      get: key => request(() => structuredClone(tables[name].get(key))),
      put: (value, key = value.key) => request(() => tables[name].set(key, structuredClone(value))),
      clear: () => request(() => tables[name].clear()),
      openCursor: () => cursor([...tables.profiles.keys()].sort()),
      index: () => ({ openCursor: () => cursor([...tables.profiles.keys()].sort((a, b) => tables.profiles.get(a).at - tables.profiles.get(b).at || a.localeCompare(b))) }),
    }); check(); return tx;
  } };
  const factory = { open: () => { const req = {}; queueMicrotask(() => { req.result = db; req.onsuccess?.(); }); return req; } };
  return { state, events, set abortWrites(value) { abortWrites = value; }, storage: archiveModule.createIndexedEditionStorage(() => factory) };
}
test('IndexedDB commits checkpoints atomically, refuses failed writes, and clears only the selected profile', async () => {
  const f = indexedFixture(), first = archiveModule.createEditionArchive(f.storage), second = archiveModule.createEditionArchive(f.storage);
  const a = [...(await first.keys(namespace, [entry('local')], () => null)).values()][0], b = [...(await first.keys(otherNamespace, [entry('local')], () => null)).values()][0];
  await first.save(a, profile(456)); await second.save(b, profile(457)); assert.equal((await second.restore([a, b])).size, 2);
  f.abortWrites = true; await assert.rejects(first.save(a, profile(999))); assert.deepEqual((await second.restore([a])).get(a), profile(456));
  f.abortWrites = false; await first.clear(namespace); assert.equal((await second.restore([a, b])).size, 1); assert.equal(f.state.usage.get('total').count, 1);
});
test('IndexedDB evicts oldest cached records to respect byte/count bounds, including records from earlier profiles', async () => {
  const f = indexedFixture(), archive = archiveModule.createEditionArchive(f.storage);
  f.state.profiles.set('old-a', { key: 'old-a', at: 1, weight: 16 * 1024 * 1024 }); f.state.profiles.set('old-b', { key: 'old-b', at: 2, weight: 16 * 1024 * 1024 });
  f.state.usage.set('total', { count: 2, bytes: 32 * 1024 * 1024 });
  const key = [...(await archive.keys(namespace, [entry('local')], () => null)).values()][0]; await archive.save(key, profile(456));
  assert.equal(f.state.profiles.has('old-a'), false); assert.equal(f.state.profiles.has('old-b'), true); assert.ok(f.state.usage.get('total').bytes <= 32 * 1024 * 1024); assert.equal((await archive.restore([key])).size, 1);
});
test('IndexedDB also enforces the record count bound when cached observations are small', async () => {
  const f = indexedFixture(), archive = archiveModule.createEditionArchive(f.storage);
  for (let i = 0; i < 16000; i++) f.state.profiles.set('old-' + i, { key: 'old-' + i, at: i, weight: 10 });
  f.state.usage.set('total', { count: 16000, bytes: 160000 });
  const key = [...(await archive.keys(namespace, [entry('local')], () => null)).values()][0]; await archive.save(key, profile(456));
  assert.equal(f.state.profiles.size, 16000); assert.equal(f.state.usage.get('total').count, 16000); assert.equal(f.state.profiles.has('old-0'), false); assert.equal((await archive.restore([key])).size, 1);
});
test('an unavailable IndexedDB reports a recoverable persistence failure instead of hanging analysis', async () => {
  const store = archiveModule.createIndexedEditionStorage(() => { throw Error('unavailable'); }); await assert.rejects(store.get([]));
});

test('per-source read scopes survive tab switching and unrelated source revisions without crossing observations',async()=>{
 const disk=storage(),f=service(disk),identity={};
 const local=f.editionReadScope(config,identity,0,0,'local'),plex=f.editionReadScope(config,identity,0,0,'plex');assert.notEqual(local,plex);
 assert.equal(f.editionReadScope(config,identity,0,0,'local'),local);assert.equal(f.editionReadScope(config,identity,0,0,'plex'),plex);
 assert.equal(f.editionReadScope(config,{},42,0,'local'),local);assert.equal(f.editionReadScope(config,identity,42,42,'plex'),plex);
 await assert.rejects(f.restoreEditionProfiles([entry('plex')],local,()=>true),e=>e==='EDITIONS_SOURCE');await assert.rejects(f.readEditionProfile(entry('local'),plex,()=>true),e=>e==='EDITIONS_SOURCE');assert.deepEqual(f.calls,[]);
});
test('source reanalysis deletes only that tab observations and reuses legacy durable records without a migration scan',async()=>{
 const disk=storage(),legacy=await restoredService(disk,[entry('local'),entry('plex')]);
 for(const e of [entry('local'),entry('plex')])await legacy.readEditionProfile(e,legacy.scope,()=>true);
 const f=service(disk),identity={},local=f.editionReadScope(config,identity,0,0,'local'),plex=f.editionReadScope(config,identity,0,0,'plex');
 assert.equal((await f.restoreEditionProfiles([entry('local')],local,()=>true)).size,1);assert.equal((await f.restoreEditionProfiles([entry('plex')],plex,()=>true)).size,1);assert.deepEqual(f.calls,[]);
 f.invalidateEditionProfiles(local);assert.equal((await f.restoreEditionProfiles([entry('local')],local,()=>true)).size,0);
 const reopened=service(disk),other=reopened.editionReadScope(config,identity,0,0,'plex');assert.equal((await reopened.restoreEditionProfiles([entry('plex')],other,()=>true)).size,1);assert.deepEqual(reopened.calls,[]);
});
test('IndexedDB selective deletion preserves other source records and rejects foreign namespaces',async()=>{
 const f=indexedFixture(),archive=archiveModule.createEditionArchive(f.storage),keys=await archive.keys(namespace,[entry('local'),entry('plex')],()=>null),values=[...keys.values()];
 for(const key of values)await archive.save(key,profile(456));await archive.clear(namespace,[values[0]]);assert.equal((await archive.restore(values)).size,1);assert.equal(f.state.usage.get('total').count,1);
 await assert.rejects(archive.clear(otherNamespace,[values[1]]));assert.equal((await archive.restore(values)).size,1);
});
