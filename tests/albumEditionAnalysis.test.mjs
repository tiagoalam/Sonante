import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
function compile(file, deps = {}) {
  const exports = {}; new Function('require', 'exports', ts.transpileModule(readFileSync(new URL('../src/' + file, import.meta.url), 'utf8'), { fileName: file, compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(name => { assert.ok(name in deps, name); return deps[name]; }, exports); return exports;
}
const sizes = compile('utils/albumEditionSizes.ts'), groups = compile('utils/albumEditions.ts');
const analysis = compile('utils/albumEditionAnalysis.ts', { './albumEditionSizes': sizes, './albumEditions': groups });
const entry = (key, title = 'Kind of Blue', source = 'local') => ({ key, source, origin: '', album: { id: key, title, artist: 'Miles Davis', track_count: 2, song_count: 2 }, connectionId: 'connection' });
const group = entries => ({ key: 'work', title: 'Kind of Blue', artist: 'Miles Davis', entries });
const profile = (bytes = [100, 200], titles = ['So What', 'Freddie Freeloader']) => ({ sizes: bytes, titles, cover_id: null });
test('different sizes, years, source IDs and unavailable observations alone never establish another edition', () => {
  const a = entry('a'), b = entry('b'); a.album.year = 1959; b.album.year = 1997;
  const result = analysis.analyzeEditionGroup(group([a, b]), new Map([['a', profile()], ['b', profile([150, 150])]]));
  assert.equal(result.variants.length, 2); assert.deepEqual(result.reasons, []);
  assert.deepEqual(analysis.analyzeEditionGroup(group([a, b]), new Map()).reasons, []);
});
test('explicit edition labels and complete different track selections are independent clues, never proof of mastering', () => {
  const a = entry('a'), b = entry('b', 'Kind of Blue [Deluxe Edition]');
  assert.deepEqual(analysis.analyzeEditionGroup(group([a, b]), new Map()).reasons, ['title']);
  b.album.title = a.album.title;
  const profiles = new Map([['a', profile()], ['b', profile([150, 150], ['So What', 'Blue in Green'])]]);
  assert.deepEqual(analysis.analyzeEditionGroup(group([a, b]), profiles).reasons, ['tracks']);
  profiles.set('b', profile([150, 150], ['FREDDIE FREELOADER', ' So  What ']));
  assert.deepEqual(analysis.analyzeEditionGroup(group([a, b]), profiles).reasons, []);
});
test('matching byte inventories group sources even if tag spelling or edition labels differ', () => {
  const a = entry('a'), b = entry('b', 'Kind of Blue (Remastered)', 'plex');
  const result = analysis.analyzeEditionGroup(group([a, b]), new Map([['a', profile()], ['b', profile([200, 100])]]));
  assert.equal(result.variants.length, 1); assert.deepEqual(result.reasons, []); assert.deepEqual(result.variants[0].entries, [a, b]);
});
test('unknown track tags and incomplete or unsafe inventories do not invent track differences or byte equality', () => {
  assert.equal(analysis.editionProfile({ sizes: null, titles: ['[Unknown Track]'], cover_id: null }), null);
  assert.equal(analysis.editionProfile({ sizes: [100, 200], titles: ['One'], cover_id: null }), null);
  assert.equal(analysis.editionProfile({ sizes: [0, 200], titles: null, cover_id: null }), null);
  assert.equal(analysis.editionProfile({ sizes: [100], titles: ['One'], cover_id: 'https://secret.invalid/?token=value' }).cover_id, null);
  assert.deepEqual(analysis.editionProfile({ sizes: null, titles: ['One'], cover_id: 'al-123', secret: 'discarded' }), { sizes: null, titles: ['One'], cover_id: 'al-123' });
});
test('session cache reuses exact metadata, scopes identities and catalogs, expires, and never stores cancellation or transient failures', async () => {
  let time = 1000, reads = 0, live = true; const cache = analysis.createEditionProfileCache(() => time), scope = {}, a = entry('a');
  const load = async () => { reads++; return profile(); };
  const read = (e = a, s = scope) => cache.read(e, s, () => live, load);
  const first = await read(); first.sizes[0] = 999; // Returned values cannot corrupt future results.
  assert.equal((await read()).sizes[0], 100); assert.equal(reads, 1);
  await read({ ...a, album: { ...a.album, year: 1960 } }); assert.equal(reads, 2);
  await read(a, {}); assert.equal(reads, 3);
  time += 300_000; await read(); assert.equal(reads, 4);
  cache.clear(scope); await read(); assert.equal(reads, 5);
  live = false; await assert.rejects(read(), error => error === 'EDITIONS_CANCELLED'); assert.equal(reads, 5);
  live = true; const unknown = entry('unknown');
  for (let i = 0; i < 2; i++) await cache.read(unknown, scope, () => true, async () => { reads++; return null; });
  assert.equal(reads, 7);
  await cache.read(entry('cancelled'), scope, () => live, async () => { live = false; return profile(); }).catch(e => assert.equal(e, 'EDITIONS_CANCELLED'));
  live = true; await read(entry('cancelled')); assert.equal(reads, 8);
});
test('partial metadata expires early and cached profile storage is bounded', async () => {
  let time = 1000, reads = 0; const cache = analysis.createEditionProfileCache(() => time), scope = {};
  const read = e => cache.read(e, scope, () => true, async () => { reads++; return profile(null); });
  await read(entry('first')); await read(entry('first')); assert.equal(reads, 1);
  time += 30_000; await read(entry('first')); assert.equal(reads, 2);
  for (let i = 0; i < 129; i++) await read(entry('item-' + i));
  await read(entry('first')); assert.equal(reads, 132);
});
test('finite album comparison deduplicates identities, limits concurrency, and fences replies after navigation', async () => {
  const entries = Array.from({ length: 20 }, (_, i) => entry(String(i))); let running = 0, peak = 0, reads = 0, live = true;
  const published = [];
  await analysis.compareEditionProfiles([...entries, entries[0]], async e => { peak = Math.max(peak, ++running); reads++; await Promise.resolve(); running--; return profile(); }, () => live, e => published.push(e.key));
  assert.equal(peak, 2); assert.equal(reads, 20); assert.equal(published.length, 20);
  published.length = 0; reads = 0;
  await analysis.compareEditionProfiles(entries, async () => { reads++; live = false; return profile(); }, () => live, e => published.push(e.key));
  assert.equal(reads, 1); assert.deepEqual(published, []);
});

test('one unavailable album profile does not stop the other sources and cannot establish equality', async () => {
  const entries = [entry('a'), entry('offline'), entry('b')], published = new Map();
  await analysis.compareEditionProfiles(entries, async e => { if (e.key === 'offline') throw 'UNAVAILABLE'; return profile(); }, () => true, (entry, value) => published.set(entry.key, value));
  assert.equal(published.size, 3); assert.equal(published.get('offline'), null);
  const result = analysis.analyzeEditionGroup(group(entries), published);
  assert.equal(result.variants.length, 2); assert.deepEqual(result.reasons, []);
});

test('copy-only exclusion needs a complete matching file inventory for every occurrence, never just titles, totals or a first batch', () => {
  const entries = [entry('local'), entry('plex', 'Kind of Blue (Remastered)', 'plex'), entry('nav', 'Kind of Blue', 'navidrome')];
  const profiles = new Map([['local', profile()], ['plex', profile([200, 100])]]);
  assert.equal(analysis.analyzeEditionGroup(group(entries), profiles).copiesOnly, false);
  profiles.set('nav', profile()); assert.equal(analysis.analyzeEditionGroup(group(entries), profiles).copiesOnly, true);
  profiles.set('nav', profile([150, 150])); assert.equal(analysis.analyzeEditionGroup(group(entries), profiles).copiesOnly, false);
  profiles.set('nav', profile(null)); assert.equal(analysis.analyzeEditionGroup(group(entries), profiles).copiesOnly, false);
  assert.equal(analysis.analyzeEditionGroup(group([entries[0]]), profiles).copiesOnly, false);
});
test('cache-only lookups reuse exact evidence without I/O, exposing mutation, extending expiry or crossing metadata/session scopes', async () => {
  let time = 1000, calls = 0; const cache = analysis.createEditionProfileCache(() => time), scope = {}, e = entry('a');
  assert.equal(cache.peek(e, scope), null);
  await cache.read(e, scope, () => true, async () => { calls++; return profile(); });
  const cached = cache.peek(e, scope); cached.sizes[0] = 999; assert.equal(cache.peek(e, scope).sizes[0], 100);
  assert.equal(cache.peek({ ...e, album: { ...e.album, year: 2013 } }, scope), null);
  assert.equal(cache.peek(e, {}), null);
  time += 299999; assert.ok(cache.peek(e, scope)); time++; assert.equal(cache.peek(e, scope), null); assert.equal(calls, 1);
});

test('an album enters the list only with two measured byte profiles and edition/track evidence; unknown members cannot be the second option', () => {
  const a = entry('a'), b = entry('b', 'Kind of Blue (Deluxe Edition)'), c = entry('c', 'Kind of Blue (Deluxe Edition)', 'plex');
  const values = new Map([['a', profile()], ['b', profile([100, 201])], ['c', null]]);
  assert.equal(analysis.verifiedEditionGroup(group([a, b, c]), values).group.entries.length, 2);
  values.set('b', profile()); assert.equal(analysis.verifiedEditionGroup(group([a, b, c]), values), null);
  values.set('b', null); assert.equal(analysis.verifiedEditionGroup(group([a, b, c]), values), null);
});
test('the finite comparison samples distinct labels before repeated copies so a real edition is not buried after eight duplicates', () => {
  const entries = Array.from({ length: 30 }, (_, i) => entry('copy-' + i)); entries.push(entry('expanded', 'Kind of Blue (Expanded Edition)'));
  const picked = analysis.editionComparisonEntries(group(entries), 8);
  assert.equal(picked.length, 8); assert.ok(picked.some(e => e.key === 'expanded'));
  assert.deepEqual(analysis.editionComparisonEntries(group(entries), 16).slice(0, 8), picked);
});
test('candidate checking never exposes copies, limits calls per group, and cancellation prevents the following albums', async () => {
  const groups = [group([entry('a'), entry('b', 'Kind of Blue (Deluxe Edition)')]), { ...group([entry('c'), entry('d', 'Kind of Blue (Deluxe Edition)')]), key: 'other' }];
  const published = []; let live = true, calls = 0;
  await analysis.checkEditionCandidates(groups, async () => { calls++; return profile(); }, () => live, (group, match, unavailable) => { published.push([match, unavailable]); live = false; });
  assert.equal(calls, 2); assert.deepEqual(published, [[[], false]]);
  assert.notEqual(analysis.editionCandidateKey(groups[0]), analysis.editionCandidateKey({ ...groups[0], entries: [groups[0].entries[0]] }));
});

test('failed comparison diagnostics preserve exact missing source occurrences without fabricating a cause', async () => {
  const a = entry('a'), b = entry('b', 'Kind of Blue (Deluxe Edition)', 'plex'); let result;
  await analysis.checkEditionCandidates([group([a, b])], async e => e.key === 'b' ? profile(null) : profile(), () => true, (...args) => { result = args; });
  assert.equal(result[2], true); assert.deepEqual(result[3].map(e => e.key), ['b']); assert.equal(result[3][0].source, 'plex');
});
test('retry retires only the incomplete session entries while retaining successful measurements', async () => {
  const cache = analysis.createEditionProfileCache(), scope = {}, a = entry('a'), b = entry('b'); let calls = 0;
  const read = e => cache.read(e, scope, () => true, async () => { calls++; return e.key === 'b' && calls < 3 ? profile(null) : profile(); });
  await read(a); await read(b); cache.forget([b], scope); await read(a); assert.equal(calls, 2); await read(b); assert.equal(calls, 3); assert.deepEqual(cache.peek(b, scope).sizes, [100, 200]);
});

test('the reported Aja and dated UHQR titles share a work without changing source metadata or weakening copy exclusion', () => {
  const titles = ['Aja', 'Aja (UHQR)', 'Aja [1980 MFSL LP]'];
  const entries = titles.map((title, i) => ({ ...entry(String(i), title), album: { ...entry(String(i), title).album, artist: 'Steely Dan' } }));
  assert.deepEqual(entries.map(e => groups.editionBaseTitle(e.album.title)), ['Aja', 'Aja', 'Aja']);
  assert.equal(groups.groupAlbumEditions(entries).length, 1);
  assert.equal(groups.editionBaseTitle("1972 - Can't Buy A Thrill (24-192 - 2022 AP UHQR - Darkivist)"), "Can't Buy A Thrill");
  const observations = new Map(entries.map((e, i) => [e.key, profile([100, 200 + i])]));
  assert.equal(analysis.verifiedEditionGroup({ ...group(entries), title: 'Aja', artist: 'Steely Dan' }, observations).group.entries.length, 3);
  assert.equal(analysis.verifiedEditionGroup(group(entries), new Map(entries.map(e => [e.key, profile()]))), null);
  for (const title of ['Aja (Live)', 'Aja (UHQR Live)', 'Aja (UHQR - Remix)', 'Aja (Something Else)', 'Aja [1980 LP]', 'Aja (UHQR - Another Album)', '1972 - Aja']) assert.equal(groups.editionBaseTitle(title), title);
});

test('work identity tolerates spelling format changes but preserves meaningful title words and numbers',()=>{
 const names=['Construção','Deus lhe Pague','Cotidiano','Samba de Orly','Valsinha'];
 assert.equal(analysis.sameAlbumTracks(names,['01. CONSTRUCAO','Deus lhe pague!','Cotidiano','Samba-de-Orly','Valsinha (2010 Remastered)']),true);
 assert.equal(analysis.editionTrackName('1972 - Soul (Remastered)'),'1972 soul');
 assert.equal(analysis.sameAlbumTracks(['Song part 1','Song part 2','Other'],['Song part 4','Song part 5','Other']),false);
 assert.equal(analysis.sameAlbumTracks(['A song','Another song','Last song'],['A song (Live)','Another song (Remix)','Last song']),false);
});
test('at least seventy percent of the smaller ordinary album must match, one occurrence at a time',()=>{
 const smaller=Array.from({length:10},(_,i)=>'Song '+i),larger=[...smaller.slice(0,7),...Array.from({length:12},(_,i)=>'Extra '+i)];
 assert.equal(analysis.sameAlbumTracks(smaller,larger),true);assert.equal(analysis.sameAlbumTracks(smaller,larger.slice(1)),false);
 assert.equal(analysis.sameAlbumTracks(['Intro','Intro','Intro','Intro','Song'],['Intro','A','B','C','D']),false);
 assert.equal(analysis.sameAlbumTracks(null,smaller),false);
});
test('short albums require complete correspondence and very short excerpts cannot identify a larger release',()=>{
 assert.equal(analysis.sameAlbumTracks(['One','Two','Three'],['ONE','Two','Three']),true);
 assert.equal(analysis.sameAlbumTracks(['One','Two','Three'],['One','Two','Three','Four','Five']),false);
 assert.equal(analysis.sameAlbumTracks(['One','Two','Three','Four'],['One','Two','Three','Other']),false);
 assert.equal(analysis.sameAlbumTracks(['One','Two','Three','Four'],['One','Two','Three','Four','Bonus']),true);
});
test('unrelated homonymous Chico Buarque albums never become editions merely because track counts differ',()=>{
 const a=entry('1978','Chico Buarque'),b=entry('1984','Chico Buarque (Remastered)');a.album.artist=b.album.artist='Chico Buarque';
 const profiles=new Map([['1978',profile(Array.from({length:11},(_,i)=>100+i),Array.from({length:11},(_,i)=>'1978 Song '+i))],['1984',profile(Array.from({length:10},(_,i)=>200+i),Array.from({length:10},(_,i)=>'1984 Song '+i))]]);
 assert.deepEqual(analysis.verifiedEditionGroups(group([a,b]),profiles),[]);
});
test('homonymous work families remain separate and a compilation cannot bridge their unrelated track selections',()=>{
 const entries=['a-base','a-expanded','b-base','b-remaster','compilation'].map(id=>entry(id,id.includes('expanded')?'Album (Expanded Edition)':id.includes('remaster')?'Album (Remastered)':id==='compilation'?'Album (Deluxe Edition)':'Album'));
 const a=Array.from({length:6},(_,i)=>'A Song '+i),b=Array.from({length:6},(_,i)=>'B Song '+i),profiles=new Map(entries.map((e,index)=>[e.key,profile(Array.from({length:e.key==='compilation'?12:6},(_,i)=>100+i+index),e.key==='compilation'?[...a,...b]:e.key.startsWith('a')?a:b)]));
 const families=analysis.verifiedEditionGroups(group(entries),profiles);assert.equal(families.length,2);assert.ok(families.every(f=>f.group.entries.length===2));assert.ok(families.every(f=>!f.group.entries.some(e=>e.key==='compilation')));
 assert.notEqual(families[0].group.key,families[1].group.key);
});
test('an explicit edition label with unavailable track names remains inconclusive',()=>{
 const a=entry('a'),b=entry('b','Kind of Blue (UHQR)');assert.equal(analysis.verifiedEditionGroup(group([a,b]),new Map([['a',profile()],['b',profile([100,201],null)]])),null);
});

test('Plex folder suffix evidence recovers normalized titles, but never unrelated folders or equal-byte copies',()=>{
 const a=entry('a','Kind of Blue','plex'),b=entry('b','Kind of Blue','plex');
 const profiles=new Map([['a',{...profile(),edition_title:'Kind of Blue'}],['b',{...profile([300,400]),edition_title:'Kind of Blue (UHQR)'}]]);
 assert.equal(analysis.verifiedEditionGroups(group([a,b]),profiles).length,1);
 assert.equal(analysis.editionDisplayTitle(b,profiles.get('b')),'Kind of Blue (UHQR)');
 for(const label of ['Unrelated (UHQR)','Some random folder','Kind of Blue (Live)']){
  profiles.set('b',{...profile([300,400]),edition_title:label});assert.equal(analysis.verifiedEditionGroups(group([a,b]),profiles).length,0);
 }
 profiles.set('b',{...profile(),edition_title:'Kind of Blue (UHQR)'});assert.equal(analysis.verifiedEditionGroups(group([a,b]),profiles).length,0);
});
test('profile projection rejects paths and arbitrary fields while retaining a bounded descriptive edition title',()=>{
 for(const edition_title of ['/private/Album (UHQR)','C:\\private\\Album (UHQR)','bad\nvalue','x'.repeat(1025)]) assert.equal(analysis.editionProfile({...profile(),edition_title}).edition_title,undefined);
 assert.equal(analysis.editionProfile({...profile(),edition_title:'Kind of Blue (UHQR)',secret:'must not persist'}).edition_title,'Kind of Blue (UHQR)');
 assert.equal('secret' in analysis.editionProfile({...profile(),secret:'must not persist'}),false);
});
