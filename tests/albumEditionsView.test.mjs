import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import ts from 'typescript';
const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(item => nodes(item, predicate)) : !tree || typeof tree !== 'object' ? []
  : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
const text = tree => Array.isArray(tree) ? tree.map(text).join(' ') : typeof tree === 'string' || typeof tree === 'number' ? String(tree) : tree?.props ? text(tree.props.children) : '';
const flush = async () => { for (let i = 0; i < 400; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
function compile(file, deps, env = {}) { const exports = {}; new Function('require', 'exports', ...Object.keys(env), ts.transpileModule(readFileSync(new URL('../src/' + file, import.meta.url), 'utf8'), { fileName: file, compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports, ...Object.values(env)); return exports; }
const helpers = compile('utils/albumEditions.ts', {});
const analysis = compile('utils/albumEditionAnalysis.ts', { './albumEditionSizes': compile('utils/albumEditionSizes.ts', {}), './albumEditions': helpers });
const base = { title: 'Album', artist: 'Artist' };
const entry = (source, id, title = 'Album') => ({ key: JSON.stringify([source, id]), source, origin: source + ' library', album: { ...base, title, id, rating_key: id, folder_path: 'Library/' + id, server_id: 'server', library_id: '7', track_count: 2, discs: [] }, serverId: 'server', libraryId: '7', accountId: 'account', connectionId: 'proxy' });
function fixture(options = {}) {
  const dictionary = JSON.parse(readFileSync(new URL('../src/locales/pt-BR.json', import.meta.url), 'utf8'));
  const t = (key, vars = {}) => {
    const lookup = path => path.split('.').reduce((value, part) => value?.[part], dictionary);
    return String(lookup(key) ?? (typeof vars.count === 'number' ? lookup(key + (vars.count === 1 ? '_one' : '_other')) : null) ?? key).replace(/\{\{(\w+)\}\}/g, (_, name) => vars[name] ?? '');
  };
  const session = options.session ?? { cache: analysis.createEditionProfileCache(), scopeState: null };
  let current; const calls = [], profileCalls = [], observers = [], scrollListeners = new Set();
  const scrollRoot = { scrollTop: 0, scrollHeight: 4000, clientHeight: 400, addEventListener: (_event, callback) => scrollListeners.add(callback), removeEventListener: (_event, callback) => scrollListeners.delete(callback) };
  class Observer {
    constructor(callback, settings) { this.callback = callback; this.settings = settings; observers.push(this); }
    observe(marker) { this.marker = marker; }
    disconnect() { this.disconnected = true; }
    intersect() { this.callback([{ isIntersecting: true }]); }
  }
  const same = (a, b) => !!a && a.length === b?.length && a.every((value, index) => Object.is(value, b[index]));
  const hooks = { ...React,
    useState(initial) { const host = current, index = host.cursor++; host.slots[index] ??= { value: typeof initial === 'function' ? initial() : initial }; return [host.slots[index].value, next => { assert.equal(host.closed, false); host.slots[index].value = typeof next === 'function' ? next(host.slots[index].value) : next; }]; },
    useRef(value) { return current.slots[current.cursor++] ??= { current: value }; },
    useMemo(fn, deps) { const host = current, index = host.cursor++; if (!same(host.slots[index]?.deps, deps)) host.slots[index] = { deps, value: fn() }; return host.slots[index].value; },
    useEffect(fn, deps) { const host = current, index = host.cursor++, old = host.slots[index]; if (!same(old?.deps, deps)) host.effects.push(() => { old?.cleanup?.(); host.slots[index] = { deps, cleanup: fn() }; }); },
  };
  const Tabs = () => null, Card = () => null, Local = () => null, Plex = () => null, Nav = () => null, Artist = () => null;
  const deps = { react: hooks, 'react/jsx-runtime': jsx, 'lucide-react': icons, 'react-i18next': { useTranslation: () => ({ t }) },
    '../utils/musicSources': { sourceEnabled: (config, kind) => config.sources[kind] }, '../utils/albumEditions': helpers, '../utils/albumEditionAnalysis': analysis,
    '../services/albumEditions': {
      editionReadScope: (config, identity, nav, local) => { const signature = JSON.stringify([config.sources, config.local_folders, nav, local]); if (!session.scopeState || session.scopeState.identity !== identity || session.scopeState.signature !== signature) session.scopeState = { identity, signature, scope: {} }; return session.scopeState.scope; },
      invalidateEditionProfiles: scope => session.cache.clear(scope),
      retryEditionProfiles: (entries, scope) => session.cache.forget(entries, scope),
      editionArchiveAvailable: () => !options.storageIssue,
      restoreEditionProfiles: async (entries, scope, current) => options.restore ? options.restore(entries, scope, current) : new Map(entries.flatMap(entry => { const profile = session.cache.peek(entry, scope); return profile ? [[entry.key, profile]] : []; })),
      cachedEditionProfiles: (entries, scope) => new Map(entries.flatMap(entry => { const profile = session.cache.peek(entry, scope); return profile ? [[entry.key, profile]] : []; })),
      readEditionProfile: async (entry, scope, current) => session.cache.read(entry, scope, current, async () => { profileCalls.push(entry.key); if (options.readProfile) return options.readProfile(entry, current); const sizes = options.readSizes ? await options.readSizes(entry) : entry.album.title.includes('Deluxe') ? [123, 457] : [123, 456]; return sizes ? { sizes, titles: ['Track A', 'Track B'], cover_id: null } : null; }), loadEditionSource: async (...args) => { calls.push(args[0]); return options.load ? options.load(...args) : { source: args[0], phase: 'ready', entries: [entry(args[0], 'one'), entry(args[0], 'two', 'Album (Deluxe Edition)')] }; } },
    './ExperimentalFeatures': { BetaBadge: () => null, ExperimentalNotice: () => null },
    './AlbumABComparison': { AlbumABComparison: () => null },
    './LibrarySourceTabs': { LibrarySourceTabs: Tabs },
    './EditionCard': { EditionCard: Card }, './EditionArtwork': { EditionArtwork: () => null }, './AlbumView': { AlbumView: Plex }, './ArtistView': { ArtistView: Artist }, './LocalBrowserView': { LocalBrowserView: Local }, './deferredViews': { NavidromeView: Nav },
  };
  const exported = compile('components/AlbumEditionsView.tsx', deps, { requestAnimationFrame: fn => fn(), IntersectionObserver: options.observe ? Observer : undefined });
  const View = options.wrapper ? exported.AlbumEditionsView : exported.SourceAlbumEditionsView;
  const host = { slots: [], effects: [], cursor: 0, closed: false,
    props: { config: { sources: { local: true, plex: true, navidrome: true }, local_folders: ['fixture'], online_artwork_enabled: false }, active: true, available: true, source: 'local', status: { state: 'play', is_updating: false }, sourceIdentity: {}, navidromeRevision: 0, catalogRefreshRevision: 0, ...options.props },
    paint() { let result; for (let i = 0; i < 3; i++) { host.cursor = 0; host.effects = []; current = host; result = View(host.props); current = null;
      if (options.scroll) for (const node of nodes(result, n => n.props?.ref)) {
        if (node.props.className?.includes('overflow-y-auto p-8')) node.props.ref.current = scrollRoot;
        else if (node.props['data-editions-more'] !== undefined) node.props.ref.current = { marker: true };
      }
      host.effects.splice(0).forEach(fn => fn()); } host.tree = result; return result; },
    async settle() { await flush(); host.paint(); await flush(); return host.paint(); },
    button(key) { const found = nodes(host.tree, node => node.type === 'button' && text(node).trim() === t(key))[0]; assert.ok(found, key); return found; },
    cards() { return nodes(host.tree, node => node.type === Card); },
    openGroup() { const group = nodes(host.tree, node => node.type === 'button' && node.props.className?.startsWith('group '))[0]; assert.ok(group); group.props.onClick(); host.paint(); },
    close() { host.slots.forEach(slot => slot?.cleanup?.()); host.closed = true; },
  };
  host.paint(); return { host, calls, profileCalls, t, Card, Local, Plex, Nav, Artist, hooks, session, Tabs, observers, scrollRoot, scrollListeners };
}
const rows = f => nodes(f.host.tree, node => node.type === 'button' && node.props.className?.startsWith('group '));
const setQuery = (f, value) => { nodes(f.host.tree, node => node.type === 'input')[0].props.onChange({ target: { value } }); f.host.paint(); };

const children = f => nodes(f.host.tree, n => n.type?.name === 'SourceAlbumEditionsView');
test('source tabs include only enabled sources and lazily retain separate visited views, with one active pane', () => {
  const f = fixture({ wrapper: true, props: { config: { sources: { local: true, plex: true, navidrome: false } } } });
  let tabs = nodes(f.host.tree, n => n.type === f.Tabs)[0]; assert.deepEqual(tabs.props.sources, ['local', 'plex']); assert.equal(children(f).length, 1); assert.equal(children(f)[0].props.source, 'local');
  tabs.props.onChange('plex'); f.host.paint(); assert.equal(children(f).length, 2); assert.deepEqual(children(f).map(n => [n.props.source, n.props.active]), [['local', false], ['plex', true]]);
  tabs = nodes(f.host.tree, n => n.type === f.Tabs)[0]; tabs.props.onChange('local'); f.host.paint(); assert.equal(children(f).length, 2); assert.equal(children(f)[0].props.active, true);
  f.host.props = { ...f.host.props, config: { sources: { local: false, plex: true, navidrome: false } } }; f.host.paint(); assert.equal(children(f).length, 1); assert.equal(children(f)[0].props.source, 'plex');
  f.host.props = { ...f.host.props, config: { sources: { local: false, plex: false, navidrome: false } } }; f.host.paint(); assert.equal(children(f).length, 0); assert.equal(nodes(f.host.tree, n => n.type === f.Tabs).length, 0); f.host.close();
});
test('an album shortcut selects its enabled source tab and never activates a disabled source', () => {
  const anchor = { ...base, source: 'plex', album_id: 'one' };
  const f = fixture({ wrapper: true, props: { anchor } }); assert.equal(children(f)[0].props.source, 'plex'); assert.equal(children(f)[0].props.anchor.album_id, 'one');
  f.host.props = { ...f.host.props, config: { sources: { local: true, plex: false, navidrome: false } } }; f.host.paint(); assert.equal(children(f)[0].props.source, 'local'); assert.equal(children(f)[0].props.anchor, null); f.host.close();
});
test('each active source view queries only that source and does not combine albums returned from another source', async () => {
  for (const source of ['local', 'plex', 'navidrome']) {
    const f = fixture({ props: { source } }); await f.host.settle(); assert.deepEqual(f.calls, [source]); assert.equal(f.profileCalls.length, 2); assert.equal(rows(f).length, 1);
    f.host.openGroup(); await f.host.settle(); assert.equal(f.host.cards().length, 2); assert.ok(f.host.cards().every(card => card.props.copies.every(e => e.source === source)));
    const chosen=f.host.cards()[0].props.copies[0]; f.host.cards()[0].props.onOpenCopy(chosen,{isConnected:false}); f.host.paint();
    if(source==='local'){const target=nodes(f.host.tree,n=>n.type===f.Local)[0];assert.equal(target.props.initialAlbumId,chosen.album.id);target.props.onAlbumNavigationBack();}
    if(source==='plex'){const target=nodes(f.host.tree,n=>n.type===f.Plex)[0];assert.equal(target.props.album.rating_key,chosen.album.rating_key);assert.equal(target.props.album.server_id,chosen.serverId);target.props.onBack();}
    if(source==='navidrome'){const target=nodes(f.host.tree,n=>n.type===f.Nav)[0];assert.deepEqual(target.props.initialAlbum,{account_id:chosen.accountId,id:chosen.album.id});target.props.onAlbumBack();}
    f.host.paint();assert.equal(f.host.cards().length,2);assert.deepEqual(f.calls,[source]);f.host.close();
  }
  const f = fixture({ load: async source => ({ source, phase: 'ready', entries: [entry('local', 'one'), entry('plex', 'two', 'Album (Deluxe Edition)')] }) });
  await f.host.settle(); assert.deepEqual(f.calls, ['local']); assert.equal(rows(f).length, 0); assert.deepEqual(f.profileCalls, []); f.host.close();
});
test('a version available only in another source cannot create an edition result in the selected source', async () => {
  for (const source of ['local', 'plex']) {
    const f = fixture({ props: { source }, load: async source => ({ source, phase: 'ready', entries: [entry(source, 'one', source === 'local' ? 'Album' : 'Album (Deluxe Edition)')] }) });
    await f.host.settle(); assert.equal(rows(f).length, 0); assert.equal(f.profileCalls.length, 0); assert.deepEqual(f.calls, [source]); f.host.close();
  }
});
test('the two unrelated Chico Buarque albums are excluded even when titles match and track counts differ', async () => {
  const a = entry('local', '1978', 'Chico Buarque'), b = entry('local', '1984', 'Chico Buarque'); a.album.track_count = 11; b.album.track_count = 10;
  a.album.artist = b.album.artist = 'Chico Buarque';
  const saved = new Map([a,b].map(e => [e.key, { sizes: Array.from({length:e.album.track_count},(_,i)=>100+i), titles: Array.from({length:e.album.track_count},(_,i)=>e.album.id+' Music '+i), cover_id:null }]));
  const f = fixture({ load: async source => ({ source, phase:'ready', entries:[a,b] }), restore: async()=>saved, readProfile:async()=>assert.fail('saved observations should be reused') });
  await f.host.settle(); assert.equal(rows(f).length,0);assert.deepEqual(f.profileCalls,[]);assert.equal(nodes(f.host.tree,n=>n.props?.role==='progressbar').length,0);f.host.close();
});
test('original and expanded editions remain a family when the smaller track selection is shared', async()=>{
  const entries=[entry('local','original','Album'),entry('local','expanded','Album (Expanded Edition)')];entries[0].album.track_count=8;entries[1].album.track_count=16;
  const f=fixture({load:async source=>({source,phase:'ready',entries}),readProfile:async e=>({sizes:Array.from({length:e.album.track_count},(_,i)=>100+i),titles:Array.from({length:e.album.track_count},(_,i)=>'Track '+i),cover_id:null})});
  await f.host.settle();assert.equal(rows(f).length,1);f.host.openGroup();await f.host.settle();assert.equal(f.host.cards().length,2);f.host.close();
});
test('two homonymous works with their own editions form separate rows and opening one cannot mix their cards or loop',async()=>{
  const entries=['a','b'].flatMap((work)=>[entry('local',work+'-base','Chico Buarque'),entry('local',work+'-edition','Chico Buarque (Remastered)')]);
  entries.forEach(e=>{e.album.artist='Chico Buarque';e.album.track_count=e.album.id.startsWith('a')?10:11;});
  const f=fixture({load:async source=>({source,phase:'ready',entries}),readProfile:async e=>({sizes:Array.from({length:e.album.track_count},(_,i)=>100+i+(e.album.id.endsWith('edition')?1:0)),titles:Array.from({length:e.album.track_count},(_,i)=>e.album.id[0]+' song '+i),cover_id:null})});
  await f.host.settle();assert.equal(rows(f).length,2);assert.notEqual(rows(f)[0].key,rows(f)[1].key);f.host.openGroup();for(let i=0;i<5;i++)await f.host.settle();
  assert.equal(f.host.cards().length,2);assert.ok(f.host.cards().every(card=>card.props.entry.album.id.startsWith(f.host.cards()[0].props.entry.album.id[0])));const reads=f.profileCalls.length;
  for(let i=0;i<10;i++){f.host.props={...f.host.props,status:{...f.host.props.status,elapsed:i}};await f.host.settle();}assert.equal(f.profileCalls.length,reads);f.host.close();
});
test('an exact album shortcut never opens another homonymous work editions',async()=>{
  const entries=[entry('local','1978','Chico Buarque'),entry('local','1984','Chico Buarque'),entry('local','1984-edition','Chico Buarque (Remastered)')];entries.forEach(e=>{e.album.artist='Chico Buarque';e.album.track_count=10;});
  const f=fixture({props:{anchor:{title:'Chico Buarque',artist:'Chico Buarque',source:'local',album_id:'1978'}},load:async source=>({source,phase:'ready',entries}),readProfile:async e=>({sizes:Array.from({length:10},(_,i)=>100+i+(e.album.id.endsWith('edition')?1:0)),titles:Array.from({length:10},(_,i)=>(e.album.id==='1978'?'old':'new')+' Song '+i),cover_id:null})});
  await f.host.settle();assert.equal(f.host.cards().length,0);assert.ok(text(f.host.tree).includes(f.t('editions.noComparedEditions')));f.host.close();
});
test('matching copies remain absent and no control counters or incomplete results appear after scanning',async()=>{
  const f=fixture({readSizes:async()=>[123,456]});await f.host.settle();assert.equal(rows(f).length,0);assert.doesNotMatch(text(f.host.tree),/candidatos|comparações incompletas|Análises reaproveitadas/i);assert.equal(nodes(f.host.tree,n=>n.props?.role==='progressbar').length,0);f.host.close();
});
test('query, scrolling and source detail navigation preserve the source catalog and do not repeat analysis',async()=>{
  const entries=Array.from({length:31},(_,i)=>[entry('local',i+'-a','Record '+i),entry('local',i+'-b','Record '+i+' (Deluxe Edition)')]).flat();
  const f=fixture({observe:true,scroll:true,load:async source=>({source,phase:'ready',entries})});for(let i=0;i<5;i++)await f.host.settle();assert.equal(rows(f).length,24);const reads=f.profileCalls.length;
  f.observers.at(-1).intersect();f.host.paint();assert.equal(rows(f).length,31);setQuery(f,'Record 9');await f.host.settle();assert.equal(rows(f).length,1);assert.equal(f.profileCalls.length,reads);assert.deepEqual(f.calls,['local']);
  f.host.openGroup();await f.host.settle();const chosen=f.host.cards()[0].props.copies[0];f.host.cards()[0].props.onOpenCopy(chosen,{isConnected:false});f.host.paint();const destination=nodes(f.host.tree,n=>n.type===f.Local)[0];assert.equal(destination.props.initialAlbumId,chosen.album.id);destination.props.onAlbumNavigationBack();f.host.paint();assert.equal(f.host.cards().length,2);f.host.close();
});
test('pause and leaving a source stop new reads and fence late replies',async()=>{
  const wait=deferred();const f=fixture({readProfile:async()=>wait.promise});await f.host.settle();assert.equal(f.profileCalls.length,2);f.host.button('editions.pauseAnalysis').props.onClick();f.host.paint();wait.resolve({sizes:[123,456],titles:['A','B'],cover_id:null});await f.host.settle();assert.equal(rows(f).length,0);assert.ok(text(f.host.tree).includes(f.t('editions.scanPaused')));f.host.close();
  const later=deferred(),inactive=fixture({readProfile:async()=>later.promise});await inactive.host.settle();inactive.host.props={...inactive.host.props,active:false};inactive.host.paint();later.resolve({sizes:[123,456],titles:['A','B'],cover_id:null});await inactive.host.settle();assert.equal(rows(inactive).length,0);assert.equal(inactive.profileCalls.length,2);inactive.host.close();
});
test('source catalog failure, unconfigured and empty states remain distinct and inactive views do nothing',async()=>{
  const inactive=fixture({props:{active:false}});await inactive.host.settle();assert.deepEqual(inactive.calls,[]);inactive.host.close();
  for(const phase of ['error','unconfigured','ready']){const f=fixture({load:async source=>({source,phase,entries:[]})});await f.host.settle();if(phase==='error')assert.ok(nodes(f.host.tree,n=>n.props?.role==='alert').length);if(phase==='unconfigured')assert.ok(text(f.host.tree).includes(f.t('editions.noConfiguredSources')));if(phase==='ready')assert.ok(text(f.host.tree).includes(f.t('editions.empty')));f.host.close();}
});

// Plex may normalize all edition titles and expose no album track counts.
// The first comparison must still happen; physical folder labels supply only
// recognized edition hints, after complete byte/track correspondence checks.
test('Plex normalized repeated titles are checked before deciding whether editions exist', async () => {
  const f=fixture({props:{source:'plex'},load:async()=>({source:'plex',phase:'ready',entries:[entry('plex','a'),entry('plex','b')]}),
    readProfile:async e=>({sizes:e.album.rating_key==='a'?[10,20]:[30,40],titles:['One','Two'],cover_id:null,edition_title:e.album.rating_key==='a'?'Album':'Album (UHQR)'})});
  await f.host.settle();assert.equal(f.profileCalls.length,2);assert.equal(rows(f).length,1);f.host.close();
});
test('source availability gates catalogs and comparisons, and losing readiness fences pending work',async()=>{
  const wait=deferred();const f=fixture({props:{readiness:{local:'busy',plex:'ready',navidrome:'disabled'}},readProfile:()=>wait.promise});
  await f.host.settle();assert.deepEqual(f.calls,[]);assert.deepEqual(f.profileCalls,[]);
  f.host.props={...f.host.props,readiness:{...f.host.props.readiness,local:'ready'}};await f.host.settle();assert.equal(f.calls.length,1);assert.equal(f.profileCalls.length,2);
  f.host.props={...f.host.props,readiness:{...f.host.props.readiness,local:'busy'}};f.host.paint();wait.resolve({sizes:[123,456],titles:['One','Two'],cover_id:null});await f.host.settle();assert.equal(rows(f).length,0);f.host.close();
});
test('partial catalogs never start comparisons, even when some entries were retrieved',async()=>{
 const f=fixture({props:{source:'plex'},load:async()=>({source:'plex',phase:'error',partial:true,entries:[entry('plex','a'),entry('plex','b','Album (Deluxe Edition)')]})});
 await f.host.settle();assert.deepEqual(f.profileCalls,[]);assert.equal(rows(f).length,0);f.host.close();
});
