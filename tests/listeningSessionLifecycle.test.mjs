import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import ts from 'typescript';
const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(item => nodes(item, predicate)) : !tree || typeof tree !== 'object' ? []
  : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const flush = async () => { for (let i = 0; i < 50; i++) await Promise.resolve(); };
const candidate = id => ({ id, title: 'Album ' + id, artist: 'Artist', year: 1970, genres: [], added_at: null });
const tracks = id => [0, 1].map(n => ({ title: 'Track ' + n, artist: 'Artist', album: 'Album ' + id, duration: 900, media_locator: { kind: 'local', uri: `Library/${id}/${n}.flac` } }));
function fixture() {
  const values = new Map(), listeners = new Map(), modules = new Map(), calls = [];
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => { calls.push('write'); values.set(key, value); } };
  const window = { localStorage: storage, addEventListener: (name, fn) => { if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name).add(fn); },
    removeEventListener: (name, fn) => listeners.get(name)?.delete(fn), dispatchEvent: event => { for (const fn of listeners.get(event.type) ?? []) fn(event); } };
  const dictionary = JSON.parse(readFileSync(new URL('../src/locales/pt-BR.json', import.meta.url), 'utf8'));
  const t = (key, data = {}) => String(key.split('.').reduce((value, part) => value?.[part], dictionary) ?? key).replace(/\{\{(\w+)\}\}/g, (_, name) => data[name] ?? '');
  let current;
  const same = (a, b) => !!a && a.length === b?.length && a.every((value, index) => Object.is(value, b[index]));
  const hooks = { ...React,
    useState(initial) { const host = current, index = host.cursor++; host.slots[index] ??= { value: typeof initial === 'function' ? initial() : initial }; return [host.slots[index].value, next => { host.slots[index].value = typeof next === 'function' ? next(host.slots[index].value) : next; }]; },
    useRef(value) { return current.slots[current.cursor++] ??= { current: value }; },
    useMemo(fn, deps) { const host = current, index = host.cursor++; if (!same(host.slots[index]?.deps, deps)) host.slots[index] = { deps, value: fn() }; return host.slots[index].value; },
    useEffect(fn, deps) { const host = current, index = host.cursor++, old = host.slots[index]; if (!same(old?.deps, deps)) host.effects.push(() => { old?.cleanup?.(); host.slots[index] = { deps, cleanup: fn() }; }); },
    useSyncExternalStore(_subscribe, get) { return get(); },
  };
  const host = { cursor: 0, slots: [], effects: [], failPlay: false, revision: 'initial', queue: [],
    props: { config: { sources: { local: true, plex: false, navidrome: false } }, available: true, active: true, status: { state: 'stop', current_media: null, options: null } },
    paint() { let result; for (let i = 0; i < 3; i++) { host.cursor = 0; host.effects = []; current = host; result = View(host.props); current = null; host.effects.splice(0).forEach(effect => effect()); } host.tree = result; return result; },
    async settle() { await flush(); host.paint(); await flush(); return host.paint(); },
    click(key) { const button = nodes(host.tree, node => node.type === 'button' && node.props.children === t(key))[0]
      ?? nodes(host.tree, node => node.type === 'button' && Array.isArray(node.props.children) && node.props.children.includes(t(key)))[0]; assert.ok(button, key); assert.ok(!button.props.disabled, key + ' enabled'); button.props.onClick({ currentTarget: { isConnected: false } }); return host.paint(); },
    name(value) { nodes(host.tree, node => node.type === 'input' && node.props['aria-label'] === t('sessions.name'))[0].props.onChange({ target: { value } }); host.paint(); },
    async prepare() { host.click('sessions.prepare'); await host.settle(); },
    async play() { const resume = nodes(host.tree, node => node.type === 'button' && Array.isArray(node.props.children) && node.props.children.includes(t('sessions.resume'))).length > 0; host.click(resume ? 'sessions.resume' : 'sessions.start'); await host.settle(); },
    select(value) { const button = nodes(host.tree, node => node.type === 'button' && nodes(node, item => item.type === 'h3' && item.props.children === value.name).length > 0)[0]; assert.ok(button, value.name); button.props.onClick(); host.paint(); },
    close() { host.slots.forEach(slot => slot?.cleanup?.()); },
  };
  const audioService = { playTracks: async (queue, index) => { calls.push('play'); if (host.failPlay) throw 'FAILED'; host.queue = queue; host.revision += '-play'; host.props.status = status(queue[index], index); },
    getQueueEditSnapshot: async () => { calls.push('snapshot'); return { revision: host.revision, tracks: host.queue }; } };
  const deps = { './ExperimentalFeatures': { BetaBadge: () => null, ExperimentalNotice: () => null }, react: hooks, 'react/jsx-runtime': jsx, 'react-i18next': { useTranslation: () => ({ t }) }, 'lucide-react': icons,
    '../services/audio': { audioService }, '../services/smartPlaylists': { smartPlaylistsService: { sources: async () => ({ local: true }) } },
    '../services/listeningSessions': { sessionCatalog: async () => ['a', 'b', 'c'].map(candidate), sessionAlbumTracks: async (_source, album) => tracks(album.id) },
    '../services/plex': {}, '../services/navidrome': {}, '../services/history': {}, '../utils/navidromeArtwork': {}, '../utils/localAlbumCatalogSession': {}, '../utils/localArtworkSession': {},
    '../utils/listeningSessionGenres': { sessionUsesGenreFamilies: () => false, sessionGenreValues: () => [], sessionMatchesGenre: () => true },
    '../utils/musicSources': { sourceEnabled: (config, kind) => config.sources[kind] }, './TrackArtwork': { TrackArtwork: () => null }, './SessionAlbumChoices': { SessionAlbumChoices: () => null },
    '../utils/playbackIntent': { beginPlaybackIntent: () => ({ current: () => true, cancel() {} }), isPlaybackSuperseded: error => error === 'PLAYBACK_SUPERSEDED' },
  };
  function load(file) {
    if (modules.has(file)) return modules.get(file);
    const exports = {}; modules.set(file, exports);
    const code = ts.transpileModule(readFileSync(new URL('../src/' + file, import.meta.url), 'utf8'), { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
    new Function('require', 'exports', 'window', 'requestAnimationFrame', code)(name => {
      if (name in deps) return deps[name];
      const prefix = name.startsWith('../utils/') ? 'utils/' : name.startsWith('./listeningSession') ? 'utils/' : null;
      assert.ok(prefix, name); return load(prefix + name.split('/').at(-1) + '.ts');
    }, exports, window, fn => fn()); return exports;
  }
  const helpers = load('utils/listeningSessions.ts'), store = load('utils/listeningSessionStore.ts').listeningSessionStore;
  const progress = load('utils/listeningSessionProgress.ts');
  const View = load('components/ListeningSessionsView.tsx').ListeningSessionsView;
  host.paint();
  return { host, store, progress, helpers, calls, t, load,
    scene: (id, albumId = id) => ({ id, name: 'Session ' + id, created_at: 1000, target: 3600, source: { kind: 'local' }, albums: [helpers.sessionAlbum(candidate(albumId), tracks(albumId), { kind: 'local' }, 'discovery')], cursor: 0, completed: false }) };
}
const status = (track, index) => ({ state: 'play', snapshot_consistent: true, current_media: { ...track.media_locator, queue_index: index }, options: null });
test('Play alone never saves a draft, even after progress advances; Save explicitly persists the chosen name and cursor', async () => {
  const f = fixture(); await f.host.settle(); await f.host.prepare(); f.host.name('My unsaved evening'); await f.host.play();
  assert.equal(f.calls.filter(value => value === 'play').length, 1);
  assert.deepEqual(f.store.list(), [], 'Play is not Save');
  await f.progress.observeSessionProgress(status(f.host.queue[1], 1)); f.host.paint();
  assert.deepEqual(f.store.list(), [], 'track changes cannot create saved sessions');
  f.host.click('sessions.save');
  assert.equal(f.store.list().length, 1); assert.equal(f.store.list()[0].name, 'My unsaved evening'); assert.equal(f.store.list()[0].cursor, 1);
  f.host.close();
});
test('failed Play neither saves a draft nor marks an existing session as started', async () => {
  const f = fixture(); await f.host.settle(); await f.host.prepare(); f.host.failPlay = true; await f.host.play();
  assert.deepEqual(f.store.list(), []);
  const saved = f.scene('saved'); f.store.save(saved); f.host.paint(); f.host.select(saved); await f.host.play();
  assert.deepEqual(f.store.list(), [saved]); f.host.close();
});
test('Save then Play keeps exactly one saved session and preserves an explicit rename', async () => {
  const f = fixture(); await f.host.settle(); await f.host.prepare(); f.host.name('Evening'); f.host.click('sessions.save'); await f.host.play();
  assert.equal(f.store.list().length, 1); f.host.name('Renamed'); f.host.click('sessions.save');
  await f.progress.observeSessionProgress(status(f.host.queue[1], 1)); f.host.paint();
  assert.equal(f.store.list().length, 1); assert.equal(f.store.list()[0].name, 'Renamed'); assert.equal(f.store.list()[0].cursor, 1); f.host.close();
});
test('browsing another session never moves its cursor, and returning to the playing draft shows its own cursor', async () => {
  const f = fixture(); await f.host.settle(); const a = f.scene('a'), b = f.scene('b'); f.store.save(a); f.store.save(b); f.host.paint();
  f.host.select(a); await f.host.play(); f.host.select(b);
  await f.progress.observeSessionProgress(status(f.host.queue[1], 1)); f.host.paint();
  assert.equal(f.store.list().find(s => s.id === a.id).cursor, 1); assert.equal(f.store.list().find(s => s.id === b.id).cursor, 0);
  assert.ok(nodes(f.host.tree, node => node.type === 'h2' && node.props.children === a.name).length, 'banner identifies A while B is being browsed');
  const bar = nodes(f.host.tree, node => node.props?.role === 'progressbar')[0]; assert.equal(bar.props['aria-valuenow'], 1, 'selected B stays at track 1');
  f.host.select(f.store.list().find(s => s.id === a.id));
  assert.equal(nodes(f.host.tree, node => node.props?.role === 'progressbar')[0].props['aria-valuenow'], 2); f.host.close();
});
test('draft progress survives leaving and returning to the view without becoming a saved session', async () => {
  const f = fixture(); await f.host.settle(); await f.host.prepare(); await f.host.play();
  f.host.close(); await f.progress.observeSessionProgress(status(f.host.queue[1], 1));
  f.host.slots = []; f.host.paint(); await f.host.settle();
  assert.deepEqual(f.store.list(), []); assert.equal(f.progress.getSessionPlayback().cursor, 1);
  f.host.click('sessions.showCurrent');
  assert.equal(nodes(f.host.tree, node => node.props?.role === 'progressbar')[0].props['aria-valuenow'], 2); f.host.close();
});
test('editing the playing selection creates an unsaved draft with independent progress', async () => {
  const f = fixture(); await f.host.settle(); await f.host.prepare(); await f.host.play();
  const playing = f.progress.getSessionPlayback(); assert.equal(playing.albums.length, 2);
  const remove = nodes(f.host.tree, node => node.type === 'button' && node.props['aria-label'] === f.t('sessions.removeAlbum'))[0]; remove.props.onClick(); f.host.paint();
  await f.progress.observeSessionProgress(status(f.host.queue[1], 1)); f.host.paint();
  assert.equal(f.progress.getSessionPlayback().cursor, 1);
  assert.equal(nodes(f.host.tree, node => node.props?.role === 'progressbar')[0].props['aria-valuenow'], 1);
  assert.deepEqual(f.store.list(), []); f.host.click('sessions.save'); const saved = f.store.list()[0];
  assert.notEqual(saved.id, playing.id); assert.equal(saved.cursor, 0); assert.equal(saved.started_at, undefined); f.host.close();
});
test('progress never recreates a deleted session or reorders unrelated saved cards', async () => {
  const f = fixture(); await f.host.settle(); const a = f.scene('a'), b = f.scene('b'); f.store.save(a); f.store.save(b); f.host.paint(); f.host.select(a); await f.host.play();
  assert.deepEqual(f.store.list().map(item => item.id), [b.id, a.id]);
  f.store.delete(a.id); await f.progress.observeSessionProgress(status(f.host.queue[1], 1));
  assert.deepEqual(f.store.list(), [b]); assert.equal(f.progress.getSessionPlayback().cursor, 1); f.host.close();
});
test('same id and name cannot redirect progress to a different source or album selection', async () => {
  const f = fixture(); await f.host.settle(); const a = f.scene('same'); f.store.save(a); f.host.paint(); f.host.select(a); await f.host.play();
  const changed = { ...a, albums: [f.helpers.sessionAlbum(candidate('foreign'), tracks('foreign'), { kind: 'local' }, 'discovery')] };
  f.store.save(changed); await f.progress.observeSessionProgress(status(f.host.queue[1], 1));
  assert.deepEqual(f.store.list(), [changed]);
  const otherSource = { ...a, source: { kind: 'plex', server_id: 'other', library_id: '7' } }; f.store.save(otherSource);
  assert.equal(f.store.updateProgress({ ...a, cursor: 1 }), false); assert.deepEqual(f.store.list(), [otherSource]); f.host.close();
});
test('replacing the queue at the same index detaches the session and cannot advance saved progress', async () => {
  const f = fixture(); await f.host.settle(); const a = f.scene('a'); f.store.save(a); f.host.paint(); f.host.select(a); await f.host.play();
  await f.progress.observeSessionProgress(status(f.host.queue[0], 0));
  f.host.queue = tracks('external'); f.host.revision = 'different-queue';
  await f.progress.observeSessionProgress(status(f.host.queue[0], 0));
  assert.equal(f.progress.getSessionPlayback(), null); assert.equal(f.store.list()[0].cursor, 0); f.host.close();
});
test('two sessions with identical tracks update only the one explicitly played', async () => {
  const f = fixture(); await f.host.settle(); const a = f.scene('a', 'shared'), b = f.scene('b', 'shared');
  f.store.save(a); f.store.save(b); f.host.paint(); f.host.select(a); await f.host.play(); f.host.select(b); await f.host.play();
  await f.progress.observeSessionProgress(status(f.host.queue[1], 1));
  assert.equal(f.store.list().find(s => s.id === a.id).cursor, 0); assert.equal(f.store.list().find(s => s.id === b.id).cursor, 1);
  assert.equal(f.progress.getSessionPlayback().id, b.id); f.host.close();
});
test('a completed saved session restarts only after successful Play, then records its own progress', async () => {
  const f = fixture(); await f.host.settle(); const a = { ...f.scene('a'), completed: true, cursor: 1, started_at: 1000 }; f.store.save(a); f.host.paint(); f.host.select(a);
  f.host.failPlay = true; await f.host.play(); assert.deepEqual(f.store.list(), [a]);
  f.host.failPlay = false; await f.host.play(); assert.equal(f.store.list()[0].completed, false); assert.equal(f.store.list()[0].cursor, 0);
  await f.progress.observeSessionProgress(status(f.host.queue[1], 1)); assert.equal(f.store.list()[0].cursor, 1); f.host.close();
});
test('new names come from album metadata and duplicate saves receive a distinct name without changing existing sessions', async () => {
  const f = fixture(); await f.host.settle(); await f.host.prepare();
  const initial = nodes(f.host.tree, node => node.type === 'input' && node.props['aria-label'] === f.t('sessions.name'))[0].props.value;
  assert.match(initial, /Album .* · Artist/); f.host.name('Evening'); f.host.click('sessions.save'); const original = f.store.list()[0];
  await f.host.prepare(); f.host.name('Evening'); f.host.click('sessions.save');
  assert.deepEqual(f.store.list().map(s => s.name), ['Evening (2)', 'Evening']); assert.deepEqual(f.store.list()[1], original);
  f.host.close();
});
test('full saved-session capacity never prevents playing an unsaved session', async () => {
  const f = fixture(); await f.host.settle(); for (let i = 0; i < 24; i++) f.store.save(f.scene('saved-' + i));
  f.host.paint(); await f.host.prepare(); await f.host.play(); assert.equal(f.calls.filter(value => value === 'play').length, 1);
  assert.equal(f.store.list().length, 24); assert.ok(f.progress.getSessionPlayback()); f.host.close();
});
test('saved resume keeps its recorded cursor and does not duplicate or rename the session', async () => {
  const f = fixture(); await f.host.settle(); const a = { ...f.scene('a'), started_at: 1000, cursor: 1 }; f.store.save(a); f.host.paint(); f.host.select(a);
  await f.host.play(); assert.equal(f.store.list().length, 1); assert.equal(f.store.list()[0].name, a.name);
  assert.equal(f.progress.getSessionPlayback().cursor, 1); assert.equal(f.host.props.status.current_media.queue_index, 1); f.host.close();
});
