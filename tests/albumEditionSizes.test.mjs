import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
function compile(file, deps = {}) { const exports = {}; new Function('require', 'exports', ts.transpileModule(readFileSync(new URL('../src/' + file, import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports); return exports; }
const sizes = compile('utils/albumEditionSizes.ts');
const edition = (key, source = 'local') => ({ key, source, album: { id: key, rating_key: key, title: 'Album', artist: 'Artist', folder_path: 'private/not-sent' }, serverId: 'server', libraryId: '7', accountId: 'account', connectionId: 'proxy', origin: source });
const group = entries => ({ key: 'work', title: 'Album', artist: 'Artist', entries });
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; };
test('matching individual sizes across all sources form one version with three exact opening destinations', () => {
  const entries = [edition('a'), edition('b', 'plex'), edition('c', 'navidrome')], before = JSON.stringify(entries);
  const observations = new Map(entries.map(entry => [entry.key, [111, 222, 333]]));
  const variants = sizes.editionSizeVariants(group(entries), observations);
  assert.equal(variants.length, 1); assert.deepEqual(variants[0].entries, entries); assert.equal(JSON.stringify(entries), before);
});
test('equal album totals with different individual sizes, counts, incomplete or unsafe sizes never collapse', () => {
  const entries = [edition('a'), edition('b'), edition('c'), edition('d'), edition('e'), edition('f')];
  const observations = new Map([['a', [100, 200]], ['b', [150, 150]], ['c', [300]], ['d', null], ['e', [0, 300]], ['f', [Number.MAX_SAFE_INTEGER + 1]]]);
  assert.equal(sizes.editionSizeVariants(group(entries), observations).length, 6);
  for (const value of [null, [], [1.5], [-1], ['123'], [NaN], [Infinity], [0]]) assert.equal(sizes.validFileSizes(value), false);
});
test('a real second version remains separate while copies and file-order differences are grouped', () => {
  const entries = [edition('a'), edition('b', 'plex'), edition('c')];
  const observations = new Map([['a', [123, 456]], ['b', [456, 123]], ['c', [123, 457]]]);
  const variants = sizes.editionSizeVariants(group(entries), observations);
  assert.equal(variants.length, 2); assert.deepEqual(variants[0].entries.map(e => e.key), ['a', 'b']); assert.equal(variants[1].entries[0].key, 'c');
});
test('unknown values remain individually visible until a complete observation arrives', () => {
  const entries = [edition('a'), edition('b')], observations = new Map();
  assert.equal(sizes.editionSizeVariants(group(entries), observations).length, 2);
  observations.set('a', [123]); observations.set('b', null); assert.equal(sizes.editionSizeVariants(group(entries), observations).length, 2);
  observations.set('b', [123]); assert.equal(sizes.editionSizeVariants(group(entries), observations).length, 1);
});
test('shared read budget retains slots until old work finishes, and cancels obsolete queued work before starting', async () => {
  const queue = sizes.createEditionSizeReadQueue(), first = deferred(), second = deferred(); let live = true; const calls = [];
  const a = queue(() => { calls.push('old-a'); return first.promise; }, () => true);
  const b = queue(() => { calls.push('old-b'); return second.promise; }, () => true);
  const cancelled = queue(async () => { assert.fail('obsolete request started'); }, () => live); const rejected = assert.rejects(cancelled, e => e === 'EDITIONS_CANCELLED');
  const fresh = queue(async () => { calls.push('fresh'); }, () => true);
  await tick(); assert.deepEqual(calls, ['old-a', 'old-b']); live = false; first.resolve(); await a; await rejected; await fresh; assert.deepEqual(calls, ['old-a', 'old-b', 'fresh']); second.resolve(); await b;
});
function service(invoke, queue) {
  return compile('services/albumEditions.ts', {
    '../utils/albumEditionArchive': { createIndexedEditionStorage: () => ({}), createEditionArchive: () => ({}), editionArchiveFingerprint: () => 'unused' },
    '@tauri-apps/api/core': { invoke }, '../utils/localAlbumCatalogSession': { localAlbumCatalog: { getSnapshot: () => { throw 'NO_CATALOG_EXPECTED'; } } },
    '../utils/localSourceReadQueue': { localSourceReadQueue: queue }, '../utils/albumEditionSizes': sizes,
    '../utils/albumEditionAnalysis': compile('utils/albumEditionAnalysis.ts', { './albumEditionSizes': sizes, './albumEditions': compile('utils/albumEditions.ts') }),
    '../utils/musicSources': { sourceEnabled: (config, kind) => config.sources[kind] }, './audio': {}, './plex': {}, './navidrome': {}, './smartPlaylists': {},
  });
}
test('size queries carry only exact source/album IDs, never music paths or authenticated stream URLs', async () => {
  const calls = [], queue = compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue();
  const read = service(async (...args) => { calls.push(args); return { sizes: [123], titles: ['Track'], cover_id: null }; }, queue).readEditionProfile;
  for (const kind of ['local', 'plex', 'navidrome']) await read(edition(kind, kind), {}, () => true);
  assert.deepEqual(calls.map(call => call[1].source.kind), ['local', 'plex', 'navidrome']);
  assert.ok(calls.every(call => call[0] === 'get_edition_profile')); assert.ok(!JSON.stringify(calls).includes('private/not-sent'));
  await assert.rejects(read(edition('cancelled'), {}, () => false), e => e === 'EDITIONS_CANCELLED'); assert.equal(calls.length, 3);
});
test('waiting playback control runs before the next Local byte-size query', async () => {
  const calls = [], wait = deferred(), queue = compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue();
  const read = service(async (_command, args) => { calls.push(args.source.album_id); if (args.source.album_id === 'first') await wait.promise; return { sizes: [123], titles: ['Track'], cover_id: null }; }, queue).readEditionProfile;
  const first = read(edition('first'), {}, () => true); await tick();
  const second = read(edition('second'), {}, () => true); const control = queue.control(async () => { calls.push('playback-control'); });
  wait.resolve(); await Promise.all([first, second, control]); assert.deepEqual(calls, ['first', 'playback-control', 'second']);
});

test('exact album profiles are reused across visits; source/account/catalog changes and explicit refresh retire cached evidence', async () => {
  let calls = 0; const queue = compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue();
  const f = service(async () => { calls++; return { sizes: [123], titles: ['Track'], cover_id: null }; }, queue);
  const identity = {}, cfg = { sources: { local: true, plex: true, navidrome: true }, local_folders: ['/fixture'], plex_server_id: 'server' };
  const scope = f.editionReadScope(cfg, identity, 0, 0);
  assert.equal(f.editionReadScope({ ...cfg, theme: 'zen' }, identity, 0, 0), scope);
  assert.notEqual(f.editionReadScope(cfg, identity, 0, 1), scope);
  assert.notEqual(f.editionReadScope(cfg, identity, 1, 0), scope);
  assert.notEqual(f.editionReadScope(cfg, {}, 0, 0), scope);
  assert.notEqual(f.editionReadScope({ ...cfg, sources: { ...cfg.sources, local: false } }, identity, 0, 0), scope); const e = edition('same', 'navidrome');
  await f.readEditionProfile(e, scope, () => true); await f.readEditionProfile(e, scope, () => true); assert.equal(calls, 1);
  await f.readEditionProfile({ ...e, connectionId: 'new-connection' }, scope, () => true); assert.equal(calls, 2);
  await f.readEditionProfile(e, {}, () => true); assert.equal(calls, 3);
  f.invalidateEditionProfiles(scope); await f.readEditionProfile(e, scope, () => true); assert.equal(calls, 4);
});

test('classification can reuse cached album profiles without initiating any native read', async () => {
  let calls = 0; const f = service(async () => { calls++; return { sizes: [123], titles: ['Track'], cover_id: null }; }, work => work());
  const e = edition('same'), scope = {}; assert.equal(f.cachedEditionProfiles([e], scope).size, 0); assert.equal(calls, 0);
  await f.readEditionProfile(e, scope, () => true); assert.equal(calls, 1);
  assert.deepEqual(f.cachedEditionProfiles([e], scope).get(e.key).sizes, [123]); assert.equal(calls, 1);
  assert.equal(f.cachedEditionProfiles([{ ...e, album: { ...e.album, title: 'Changed' } }], scope).size, 0);
  f.invalidateEditionProfiles(scope); assert.equal(f.cachedEditionProfiles([e], scope).size, 0); assert.equal(calls, 1);
});

test('unrelated Plex directory names are discarded before an observation can be cached or saved',async()=>{
 const queue=compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue();
 const source={...edition('id'),source:'plex',serverId:'server',libraryId:'7',album:{rating_key:'id',title:'Album',artist:'Artist'}};
 for(const label of ['private-user-folder','Other Album (UHQR)']){
  const read=service(async()=>({sizes:[123],titles:['Song'],cover_id:null,edition_title:label}),queue).readEditionProfile;
  assert.equal((await read(source,{},()=>true)).edition_title,undefined);
 }
 const read=service(async()=>({sizes:[123],titles:['Song'],cover_id:null,edition_title:'Album (UHQR)'}),queue).readEditionProfile;
 assert.equal((await read(source,{},()=>true)).edition_title,'Album (UHQR)');
});
