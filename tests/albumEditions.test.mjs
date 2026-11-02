import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
function compile(file, deps = {}, env = {}) {
  const exports = {}, source = readFileSync(new URL('../src/' + file, import.meta.url), 'utf8');
  new Function('require', 'exports', ...Object.keys(env), ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports, ...Object.values(env)); return exports;
}
const helpers = compile('utils/albumEditions.ts');
const base = { title: 'Kind of Blue', artist: 'Miles Davis' };
const local = (id, title = base.title, artist = base.artist) => ({ key: JSON.stringify(['local', id]), source: 'local', album: { id, title, artist, track_count: 5, folder_path: 'Jazz/' + id, source_label: 'Jazz', discs: [] }, origin: 'Jazz' });
test('recognized edition suffixes join one work while original metadata and separate identities remain intact', () => {
  const names = ['Kind of Blue', 'Kind of Blue (2013 Remastered)', 'Kind of Blue [Deluxe Edition]', 'Kind of Blue - 50th Anniversary Edition', 'Kind of Blue (SACD)', 'Kind of Blue [MFSL] (2010 Remaster)', 'Kind of Blue [XRCD24]', 'Kind of Blue (Mono)'];
  const entries = names.map((title, index) => local(String(index), title)); const before = JSON.stringify(entries);
  const groups = helpers.groupAlbumEditions(entries);
  assert.equal(groups.length, 1); assert.equal(groups[0].entries.length, names.length); assert.equal(JSON.stringify(entries), before);
  assert.deepEqual(new Set(groups[0].entries.map(entry => entry.album.title)), new Set(names));
});
test('live, remix, location, re-recording, disc numbers, unrelated artists, punctuation and accents are not discarded', () => {
  const different = ['Kind of Blue (Live)', 'Kind of Blue (Live Remastered)', 'Kind of Blue (Remixes)', 'Kind of Blue [Disc 1]', 'Kind of Blue (CD 1)', 'Kind of Blue (New York)', 'Kind of Blue (Re-recorded 2020)', 'Kind of Blúe', 'Kind-of-Blue'];
  for (const title of different) assert.notEqual(helpers.editionWorkKey({ ...base, title }), helpers.editionWorkKey(base), title);
  assert.notEqual(helpers.editionWorkKey({ ...base, artist: 'Miles Davis Quartet' }), helpers.editionWorkKey(base));
  assert.equal(helpers.editionWorkKey({ ...base, title: ' KIND  OF BLUE ', artist: 'MILES DAVIS' }), helpers.editionWorkKey(base));
});
test('generic or unsafe metadata cannot group unrelated albums', () => {
  for (const artist of ['', 'Unknown Artist', 'Various Artists', 'Vários artistas', 'Artist\nInjected']) assert.equal(helpers.editionWorkKey({ ...base, artist }), null);
  for (const title of ['', 'Untitled', 'Unknown Album', 'Sem título', 'x'.repeat(1025)]) assert.equal(helpers.editionWorkKey({ ...base, title }), null);
});
test('all three sources remain individually actionable even when titles, years and raw ids coincide', () => {
  const a = local('one');
  const b = { key: JSON.stringify(['plex', 'server', 'one']), source: 'plex', origin: 'Jazz', serverId: 'server', libraryId: '7', album: { ...base, rating_key: 'one', year: 1959 } };
  const c = { key: JSON.stringify(['navidrome', 'account', 'one']), source: 'navidrome', origin: '', accountId: 'account', connectionId: 'proxy', album: { ...base, id: 'one', year: 2013 } };
  const [group] = helpers.groupAlbumEditions([a, b, c, { ...b, origin: 'Another view of the same Plex album' }]);
  assert.equal(group.entries.length, 3); assert.deepEqual(new Set(group.entries.map(e => e.key)), new Set([a.key, b.key, c.key]));
  assert.equal(helpers.filterEditionGroups([group], '', 'all').length, 1); assert.equal(helpers.filterEditionGroups([group], '', 'local').length, 0);
  assert.equal(helpers.filterEditionGroups([group], 'miles', 'all').length, 1); assert.equal(helpers.filterEditionGroups([group], 'unrelated', 'all').length, 0);
});
test('opening album editions preserves only validated navigation source/album context and unsubscribes cleanly', () => {
  const events = new Map(), navigations = [];
  const window = { addEventListener: (key, fn) => events.set(key, fn), removeEventListener: key => events.delete(key), dispatchEvent: event => events.get(event.type)?.(event) };
  const tools = compile('utils/albumEditions.ts', {}, { window, CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail; } } });
  const stop = tools.subscribeAlbumEditions(value => navigations.push(value));
  tools.openAlbumEditions({ ...base, private: 'never forwarded', source: 'plex', album_id: '123' }); assert.deepEqual(navigations, [{...base,source:'plex',album_id:'123'}]);
  tools.openAlbumEditions({...base,source:'radio'}); assert.equal(navigations.length,1);
  window.dispatchEvent({ type: tools.ALBUM_EDITIONS_EVENT, detail: { title: 123, artist: 'Artist' } }); assert.equal(navigations.length, 1);
  stop(); tools.openAlbumEditions(base); assert.equal(navigations.length, 1);
});
const config = { sources: { local: true, plex: true, navidrome: true }, local_folders: ['fixture'], plex_server_id: 'server', plex_token: 'fixture' };
function serviceFixture(options = {}) {
  const calls = []; let sourceReads = 0, navReads = 0;
  const deps = {
    '../utils/albumEditionArchive': { createIndexedEditionStorage: () => ({}), createEditionArchive: () => ({}), editionArchiveFingerprint: () => 'unused' },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      assert.equal(command, 'navidrome_edition_catalog'); assert.equal(args.accountId, 'account'); assert.equal(args.connectionId, 'proxy');
      calls.push(['catalog', { kind: 'navidrome', account_id: args.accountId }]);
      if (options.navCatalog) return options.navCatalog(() => true);
      return [{ ...base, id: 'nav-one', year: 2010, cover_id: 'al-nav-one', song_count: 5 }];
    } },
    '../utils/localAlbumCatalogSession': { localAlbumCatalog: { getSnapshot: () => options.catalog ?? { loaded: true, albums: [local('one').album] } } },
    '../utils/localSourceReadQueue': { localSourceReadQueue: compile('utils/localSourceReadQueue.ts').createLocalSourceReadQueue() },
    '../utils/albumEditionSizes': compile('utils/albumEditionSizes.ts'),
    '../utils/albumEditionAnalysis': compile('utils/albumEditionAnalysis.ts', { './albumEditionSizes': compile('utils/albumEditionSizes.ts'), './albumEditions': helpers }),
    '../utils/musicSources': { sourceEnabled: (cfg, kind) => cfg.sources[kind] },
    './audio': { audioService: options.audio ?? { getLocalAlbums: async () => { calls.push('local'); return [local('one').album]; } } },
    './plex': { plexService: { getLibraries: async () => { calls.push('libraries'); return [{ key: '7', title: 'Jazz' }, { key: '8', title: 'Rock' }]; }, getAlbums: async key => { calls.push('plex:' + key); if (options.failLibrary === key) throw 'OFFLINE'; return [{ ...base, rating_key: key, year: 1959 }]; } } },
    './navidrome': { navidromeService: { status: async () => { calls.push('nav-status'); return options.navStatus?.(++navReads) ?? { connection_id: 'proxy', account_id: 'account' }; } } },
    './smartPlaylists': { smartPlaylistsService: { sources: async () => { calls.push('sources'); return options.sourceStatus?.(++sourceReads) ?? { plex_server_id: 'server' }; }, catalog: async (source, current) => { calls.push(['catalog', source]); if (options.navCatalog) return options.navCatalog(current); return [{ ...base, id: 'nav-one', year: 2010 }]; } } },
  };
  return { calls, ...compile('services/albumEditions.ts', deps) };
}
test('disabled sources do no catalog, configuration, connection or playback work', async () => {
  const f = serviceFixture();
  for (const source of ['local', 'plex', 'navidrome']) assert.equal((await f.loadEditionSource(source, { ...config, sources: { local: false, plex: false, navidrome: false } }, () => true)).phase, 'unconfigured');
  assert.deepEqual(f.calls, []);
});
test('source catalogs keep origin context, with partial Plex results retained and failed libraries identified', async () => {
  const f = serviceFixture({ failLibrary: '8' });
  const a = await f.loadEditionSource('local', config, () => true); assert.equal(a.entries[0].origin, 'Jazz');
  const b = await f.loadEditionSource('plex', config, () => true); assert.equal(b.phase, 'error'); assert.equal(b.partial, true); assert.equal(b.entries.length, 1);
  assert.equal(b.entries[0].album.server_id, 'server'); assert.equal(b.entries[0].album.library_id, '7');
  const c = await f.loadEditionSource('navidrome', config, () => true); assert.equal(c.entries[0].accountId, 'account'); assert.equal(c.entries[0].connectionId, 'proxy'); assert.equal(c.entries[0].album.cover_id, 'al-nav-one'); assert.equal(c.entries[0].album.song_count, 5);
});
test('server/account changes invalidate the whole affected lane rather than relabeling old albums', async () => {
  const plex = serviceFixture({ sourceStatus: n => ({ plex_server_id: n === 1 ? 'server' : 'different' }) });
  await assert.rejects(plex.loadEditionSource('plex', config, () => true), value => value === 'EDITIONS_SOURCE');
  const nav = serviceFixture({ navStatus: n => ({ connection_id: n === 1 ? 'proxy' : 'different', account_id: 'account' }) });
  await assert.rejects(nav.loadEditionSource('navidrome', config, () => true), value => value === 'EDITIONS_SOURCE');
});
test('cancellation fences late catalog results and prevents any subsequent source work', async () => {
  let live = false; const f = serviceFixture();
  await assert.rejects(f.loadEditionSource('local', config, () => live), value => value === 'EDITIONS_CANCELLED'); assert.deepEqual(f.calls, []);
  const pending = serviceFixture({ navCatalog: async () => { live = false; return []; } }); live = true;
  await assert.rejects(pending.loadEditionSource('navidrome', config, () => live), value => value === 'EDITIONS_CANCELLED');
  assert.equal(pending.calls.filter(call => call === 'nav-status').length, 1);
});
test('Local editions reuse the confirmed shared catalog without a second native read', async () => {
  const albums = [local('one').album];
  const f = serviceFixture({ catalog: { loaded: true, albums }, audio: { getLocalAlbums: () => assert.fail('duplicate read') } });
  const result = await f.loadEditionSource('local', config, () => true);
  assert.equal(result.entries[0].album, albums[0]); assert.deepEqual(f.calls, []);
});
test('saved, loading, failed or updating Local catalogs never admit an edition check', async () => {
  for (const patch of [{loaded:false}, {loading:true}, {error:'offline'}, {saveBlocked:true}, {mpdBusy:true}, {manualCheckPlayback:true}]) {
    const f=serviceFixture({catalog:{loaded:true,albums:[local('one').album],...patch}});
    await assert.rejects(f.loadEditionSource('local',config,()=>true),v=>v==='EDITIONS_NOT_READY'); assert.deepEqual(f.calls,[]);
  }
});

test('bracketed unknown metadata and embedded website advertising are excluded without editing source records', () => {
  const entries = [local('unknown', '[Unknown Album]', '[Unknown Artist]'), local('spam', 'acervorootsrecords.blogspot.com-MAIOR-ACERVO', 'acervorootsrecords.blogspot.com'), local('valid', 'The Who Sell Out', 'The Who')];
  const before = JSON.stringify(entries);
  assert.equal(helpers.editionMetadataIssue(entries[0].album), 'unknown');
  assert.equal(helpers.editionMetadataIssue(entries[1].album), 'suspicious');
  assert.deepEqual(helpers.groupAlbumEditions(entries).map(g => g.title), ['The Who Sell Out']);
  assert.equal(JSON.stringify(entries), before);
  assert.equal(helpers.editionMetadataIssue({ title: 'Dr. No', artist: 'Dr. John' }), null);
});
