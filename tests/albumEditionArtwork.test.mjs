import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import ts from 'typescript';
function compile(file, deps = {}, env = {}) {
  const exports = {}; new Function('require', 'exports', ...Object.keys(env), ts.transpileModule(readFileSync(new URL('../src/' + file, import.meta.url), 'utf8'), { fileName: file, compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports, ...Object.values(env)); return exports;
}
const utils = compile('utils/albumEditionArtwork.ts');
const entries = () => [
  { source: 'local', key: 'local-one', origin: 'Jazz', album: { id: 'one', title: 'Soul Station', artist: 'Hank Mobley', folder_path: 'Jazz/Soul Station', discs: [] } },
  { source: 'plex', key: 'plex-one', origin: 'Jazz', serverId: 'server', libraryId: '7', album: { rating_key: '123', title: 'Soul Station', artist: 'Hank Mobley', thumb: { server_id: 'server', path: '/library/metadata/123/thumb/1' } } },
  { source: 'navidrome', key: 'nav-one', origin: '', connectionId: 'connection', accountId: 'account', album: { id: 'nav', title: 'Soul Station', artist: 'Hank Mobley', cover_id: 'al-cover' } },
];
const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(item => nodes(item, predicate)) : !tree || typeof tree !== 'object' ? [] : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const flush = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
const deferred = () => { let resolve; return { promise: new Promise(yes => { resolve = yes; }), resolve: value => resolve(value) }; };
test('a Local-first edition uses its Plex copy cover, deduplicates references and preserves source identities', () => {
  const copies = entries(), before = JSON.stringify(copies), result = utils.editionArtworkCandidates(copies[0], [...copies, copies[1]]);
  assert.deepEqual(result.map(image => image.kind), ['plex', 'local', 'navidrome']); assert.equal(result[0].image, copies[1].album.thumb); assert.equal(JSON.stringify(copies), before);
  // The helper receives one measured edition; it never searches other variants.
  assert.deepEqual(utils.editionArtworkCandidates(copies[0], [copies[0]]).map(image => image.kind), ['local']);
});
test('Navidrome profile cover IDs are scoped to their occurrence/connection and never applied to a different copy', () => {
  const [, , nav] = entries(), other = { ...nav, key: 'other', connectionId: 'different', album: { ...nav.album, cover_id: 'other-cover' } };
  const images = utils.editionArtworkCandidates(nav, [nav, other], 'profile-cover');
  assert.deepEqual(images.map(image => [image.connectionId, image.coverId]), [['connection', 'profile-cover'], ['different', 'other-cover']]);
});
function fixture(options = {}) {
  const copies = entries(); let host;
  const calls = [], cache = new Map(options.cache ?? []), albums = options.albums ?? [copies[0].album];
  const snapshot = { albums, loaded: true, loading: false, saveBlocked: false, mpdBusy: false, error: null, ...options.catalog };
  const same = (a, b) => a?.length === b?.length && a?.every((v, i) => Object.is(v, b[i]));
  const hooks = { ...React,
    useState(initial) { const index = host.cursor++; host.slots[index] ??= { value: typeof initial === 'function' ? initial() : initial }; return [host.slots[index].value, next => { assert.equal(host.closed, false); host.slots[index].value = typeof next === 'function' ? next(host.slots[index].value) : next; }]; },
    useMemo(fn, deps) { const index = host.cursor++; if (!same(host.slots[index]?.deps, deps)) host.slots[index] = { deps, value: fn() }; return host.slots[index].value; },
    useRef(value) { return host.slots[host.cursor++] ??= { current: value }; },
    useSyncExternalStore() { return snapshot; },
    useEffect(fn, deps) { const index = host.cursor++, old = host.slots[index]; if (!same(old?.deps, deps)) host.effects.push(() => { old?.cleanup?.(); host.slots[index] = { deps, cleanup: fn() }; }); },
  };
  const Plex = () => null;
  const { EditionArtwork: View } = compile('components/EditionArtwork.tsx', {
    react: hooks, 'react/jsx-runtime': jsx, 'lucide-react': icons,
    '../utils/albumEditionArtwork': utils, './PlexImage': { PlexImage: Plex },
    '../utils/localAlbumCatalogSession': { localAlbumCatalog: { subscribe() {}, getSnapshot: () => snapshot } },
    '../utils/localArtwork': compile('utils/localArtwork.ts'),
    '../utils/localArtworkSession': { localAlbumThumbnails: { get: () => options.thumbnail ?? null }, localArtworkResolver: { get: id => cache.get(id) ?? null, resolve: async (album, online, blocked, _phase, current) => { assert.equal(online, false); assert.equal(current(), true); calls.push(['local', album.id]); assert.equal(albums.includes(album), true); assert.equal(blocked(), false); return options.local ? options.local(album, blocked) : null; } } },
    '../utils/navidromeArtwork': { navidromeAlbumCover: async (...args) => { calls.push(['navidrome', args[0], args[1]]); return options.nav ? options.nav(...args) : 'blob:nav-cover'; } },
  }, { IntersectionObserver: undefined });
  host = { cursor: 0, slots: [], effects: [], closed: false, props: { entry: copies[0], copies, active: true, updating: false, ...options.props },
    paint() { let result; for (let i = 0; i < 3; i++) { host.cursor = 0; host.effects = []; result = View(host.props); for (const node of nodes(result, node => node.props?.ref)) node.props.ref.current = {}; host.effects.splice(0).forEach(fn => fn()); } host.tree = result; return result; },
    async settle() { for (let i = 0; i < 4; i++) { await flush(); host.paint(); } return host.tree; },
    plex() { return nodes(host.tree, node => node.type === Plex)[0]; }, image() { return nodes(host.tree, node => node.type === 'img')[0]; },
    close() { host.slots.forEach(slot => slot?.cleanup?.()); host.closed = true; },
  };
  host.paint(); return { host, calls, copies, snapshot };
}
test('the reported Local/Plex edition card selects exactly the Plex reference used in its list, without requiring Local catalog admission', async () => {
  const f = fixture({ albums: [], catalog: { loaded: false } }); await f.host.settle();
  assert.equal(f.host.plex().props.image, f.copies[1].album.thumb); assert.deepEqual(f.calls, []); f.host.close();
});
test('a failed Plex image falls back to the already cached Local cover of the same edition without native reads', async () => {
  const f = fixture({ cache: [['one', 'blob:local-cached']] }); await f.host.settle();
  f.host.plex().props.onLoadError(); f.host.paint(); await f.host.settle(); assert.equal(f.host.image().props.src, 'blob:local-cached'); assert.deepEqual(f.calls, []); f.host.close();
});
test('missing or blocked Local covers can fall back to the same edition Navidrome cover without fetching tracks', async () => {
  const f = fixture({ catalog: { loading: true } }); await f.host.settle(); f.host.plex().props.onLoadError(); f.host.paint(); await f.host.settle();
  assert.equal(f.host.image().props.src, 'blob:nav-cover'); assert.deepEqual(f.calls, [['navidrome', 'connection', 'al-cover']]); f.host.close();
});
test('inactive views do no image work and obsolete Plex failures cannot replace another edition image', async () => {
  const f = fixture(); const oldFailure = f.host.plex().props.onLoadError;
  const replacement = { ...f.copies[1], key: 'plex-two', album: { ...f.copies[1].album, thumb: { server_id: 'other', path: '/different-cover' } } };
  f.host.props = { ...f.host.props, entry: replacement, copies: [replacement] }; f.host.paint(); oldFailure(); f.host.paint(); assert.equal(f.host.plex().props.image.server_id, 'other'); assert.deepEqual(f.calls, []);
  f.host.props = { ...f.host.props, active: false }; f.host.paint(); assert.equal(f.host.plex(), undefined); await f.host.settle(); assert.deepEqual(f.calls, []); f.host.close();
});
test('late image resolution cannot leak a previous Navidrome album into a new selection', async () => {
  const wait = deferred(), nav = entries()[2];
  const f = fixture({ props: { entry: nav, copies: [nav] }, nav: async (_connection, cover) => cover === 'al-cover' ? wait.promise : 'blob:new-cover' });
  await f.host.settle(); const next = { ...nav, key: 'new', album: { ...nav.album, id: 'new', cover_id: 'new-cover' } };
  f.host.props = { ...f.host.props, entry: next, copies: [next] }; f.host.paint(); await f.host.settle(); assert.equal(f.host.image().props.src, 'blob:new-cover');
  wait.resolve('blob:old-cover'); await f.host.settle(); assert.equal(f.host.image().props.src, 'blob:new-cover'); f.host.close();
});

test('Local saved thumbnails paint while a bounded full-cover lookup is pending', async () => {
  const wait=deferred(); const f=fixture({props:{entry:entries()[0],copies:[entries()[0]]},thumbnail:'data:thumbnail',local:()=>wait.promise});
  await f.host.settle(); assert.equal(f.host.image().props.src,'data:thumbnail');
  wait.resolve('blob:full-cover'); await f.host.settle(); assert.equal(f.host.image().props.src,'blob:full-cover'); f.host.close();
});
test('a saved Local thumbnail remains visible during admission blocks without native lookups',async()=>{
  const f=fixture({props:{entry:entries()[0],copies:[entries()[0]]},thumbnail:'data:thumbnail',catalog:{loaded:false}});
  await f.host.settle(); assert.equal(f.host.image().props.src,'data:thumbnail');assert.deepEqual(f.calls,[]);f.host.close();
});
