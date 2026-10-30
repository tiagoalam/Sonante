import test from "node:test";
import assert from "node:assert/strict";
import { createLocalImportPreview, importPreviewForCatalog, LOCAL_IMPORT_PREVIEW_LIMIT } from "./localImportPreview.ts";
import { createImportArtworkCache, IMPORT_ARTWORK_CACHE_BYTES } from "./importArtworkCache.ts";
import { LocalAlbumCatalog } from "./localAlbumCatalog.ts";

const flush = async () => { for (let i = 0; i < 16; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return {promise, resolve, reject}; };
const row = (n, source = "A", id = "1") => ({ provisional_group_key: n.toString(16).padStart(64, "0"), source_id: source, album: "Same", artist: "Artist", artwork_ref: `cover-v1:${id}:${n.toString(16)}`, image: null });
const sample = (albums, initial = true, id = "1") => ({operation_id: id, initial, provisional: true, playable: false, complete: false, albums});
const op = (phase = "indexing", id = "1") => ({id, phase, error: null});
const png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aXioAAAAASUVORK5CYII=";
const pngBytes = Buffer.from(png.split(",")[1], "base64").length;

function cacheFixture(budget = pngBytes * 2) {
  const requests = [], live = new Map(), revoked = [];
  let next = 0, peak = 0;
  const cache = createImportArtworkCache((id, reference) => { const d = deferred(); requests.push({id, reference, ...d}); return d.promise; }, {
    budget,
    create: data => { const bytes = Buffer.from(data.split(",")[1], "base64").length; const url = `blob:test-${++next}`; live.set(url, bytes); peak = Math.max(peak, [...live.values()].reduce((a,b) => a+b, 0)); return {url, bytes}; },
    revoke: url => { revoked.push(url); live.delete(url); },
  });
  cache.activate("1");
  return {cache, requests, live, revoked, peak: () => peak};
}

test("initial groups accumulate beyond48 across bounded batches and sources without duplicate identities", async () => {
  const requests = [], publications = [];
  const art = cacheFixture();
  const preview = createLocalImportPreview(() => { const d = deferred(); requests.push(d); return d.promise; },
    (albums, revision) => publications.push({albums, revision}), {artwork: art.cache, catalogRevision: () => 7});
  const all = [...Array.from({length: 90}, (_,i) => row(i+1,"A")), ...Array.from({length:90},(_,i)=>row(i+1000,"B"))];
  for (const batch of [all.slice(0,100), all.slice(75), all.slice(0,40)]) {
    preview.observe(op()); requests.at(-1).resolve(sample(batch.map(r => ({...r, image: png, discs:["1"], observed_tracks:999})))); await flush();
  }
  const last = publications.at(-1);
  assert.equal(last.albums.length,180); assert.equal(last.revision,7);
  assert.equal(new Set(last.albums.map(r=>r.provisional_group_key)).size,180);
  assert.ok(last.albums.every(r=>r.image===null && !("discs" in r) && !("observed_tracks" in r)));
  assert.equal(art.cache.retainedBytes(),0); // no offscreen image retention
  assert.equal(last.albums[0].provisional_group_key,all[0].provisional_group_key);
  preview.observe(op("validating")); assert.equal(publications.at(-1).albums.length,180);
  const count=publications.length; preview.dispose(); assert.equal(publications.length,count); assert.equal(art.cache.retainedBytes(),0);
});

test("byte cache eviction/reload affects only covers, never metadata, and stores no base64 in publications", async () => {
  const f=cacheFixture(); const refs=[1,2,3].map(n=>row(n).artwork_ref);
  let changes=0; const cleanup=refs.map(ref=>f.cache.watch(ref,()=>changes++));
  f.cache.tick(); f.requests[0].resolve(png); await flush();
  f.cache.tick(); f.requests[1].resolve(png); await flush();
  f.cache.tick(); f.requests[2].resolve(png); await flush();
  assert.equal(f.cache.retainedBytes(),pngBytes*2); assert.equal(f.cache.get(refs[0]),null);
  assert.ok(f.revoked.includes("blob:test-1")); assert.match(f.cache.get(refs[2]),/^blob:/);
  f.cache.tick(); assert.equal(f.requests.at(-1).reference,refs[0]); f.requests.at(-1).resolve(png); await flush();
  assert.match(f.cache.get(refs[0]),/^blob:/); assert.ok(changes>=4); assert.ok(f.peak()<=pngBytes*2);
  cleanup.forEach(stop=>stop()); f.cache.clear(); assert.equal(f.live.size,0);
  assert.equal(f.cache.retainedBytes(),0); assert.equal(IMPORT_ARTWORK_CACHE_BYTES,8*1024*1024);
});

test("cover requests are viewport-only, at most two per monitor tick, obsolete replies cannot retain URLs", async () => {
  const f=cacheFixture(); const stops=Array.from({length:10},(_,i)=>f.cache.watch(row(i+1).artwork_ref,()=>{}));
  assert.equal(f.requests.length,0); f.cache.tick();assert.equal(f.requests.length,1);
  f.requests[0].resolve(null); await flush(); assert.equal(f.requests.length,2);
  f.requests[1].reject("busy");await flush();assert.equal(f.requests.length,2);
  f.cache.tick();const delayed=f.requests.at(-1);f.cache.pause();delayed.resolve(png);await flush();assert.equal(f.live.size,0);
  f.cache.activate("2");const stale=f.cache.watch(row(9,"A","2").artwork_ref,()=>{});f.cache.tick();const old=f.requests.at(-1);f.cache.clear();old.resolve(png);await flush();assert.equal(f.live.size,0);
  stops.forEach(stop=>stop());stale();
});

test("oversize image is rejected before URL creation and unsubscribe prevents offscreen retention", async () => {
  const f=cacheFixture(pngBytes-1), ref=row(1).artwork_ref;const stop=f.cache.watch(ref,()=>{});
  f.cache.accept(ref,png);assert.equal(f.live.size,0);assert.equal(f.cache.retainedBytes(),0);
  stop();f.cache.accept(ref,png);assert.equal(f.live.size,0);f.cache.clear();
});

test("final handoff retains initial groups on idle/load failure and switches when confirmed revision advances", async () => {
  const load=deferred(), reads=[];const catalog=new LocalAlbumCatalog(()=>load.promise);
  let presentation=[];let baseline=null;
  const preview=createLocalImportPreview(()=>{const d=deferred();reads.push(d);return d.promise;},(rows,revision)=>{presentation=rows;baseline=revision??null;},{catalogRevision:catalog.getConfirmedRevision});
  const rendered=()=>[...catalog.getSnapshot().albums, ...importPreviewForCatalog(presentation,baseline,catalog.getConfirmedRevision())];
  preview.observe(op());reads[0].resolve(sample([row(1)]));await flush();
  preview.observe(op());preview.observe(op("validating"));reads[1].resolve(sample([row(2)]));await flush();assert.equal(rendered().length,1);
  preview.observe(op("completed"));assert.equal(rendered().length,1);
  assert.equal(importPreviewForCatalog(presentation,0,0),presentation);
  assert.deepEqual(importPreviewForCatalog(presentation,0,1),[]);
  catalog.setSaveBlocked(true);catalog.setUpdating(false);catalog.setSaveBlocked(false);await flush();assert.equal(rendered().length,1);
  const frames=[];const stop=catalog.subscribe(()=>{frames.push(rendered());preview.confirmCatalog(catalog.getConfirmedRevision());});
  load.resolve([{id:"confirmed",title:"Final"}]);await flush();
  assert.equal(catalog.getConfirmedRevision(),1);assert.deepEqual(catalog.getSnapshot().albums,[{id:"confirmed",title:"Final"}]);
  assert.ok(frames.every(frame=>frame.length>0));assert.deepEqual(presentation,[]);stop();preview.dispose();
  // Failed final load does not certify empty catalog or remove retained initial data.
  const fail=new LocalAlbumCatalog(async()=>{throw Error("read failed")});fail.setUpdating(false,true);await flush();
  assert.equal(fail.getConfirmedRevision(),0);assert.equal(fail.getSnapshot().error,"read failed");
});

test("initial operation failure/replacement clears all groups and late replies; addition still retains only48", async () => {
  for (const phase of ["failed","uncertain","preparing"]) {
    const reads=[];let visible=[];const preview=createLocalImportPreview(()=>{const d=deferred();reads.push(d);return d.promise;},rows=>visible=rows);
    preview.observe(op());reads[0].resolve(sample([row(1)]));await flush();preview.observe(op());
    preview.observe(op(phase,phase==="preparing"?"2":"1"));assert.deepEqual(visible,[]);
    reads[1].resolve(sample([row(2)]));await flush();assert.deepEqual(visible,[]);preview.dispose();
  }
  let visible=[];const preview=createLocalImportPreview(async()=>sample(Array.from({length:100},(_,i)=>row(i)),false),rows=>visible=rows);
  preview.observe(op());await flush();assert.equal(visible.length,LOCAL_IMPORT_PREVIEW_LIMIT);
  preview.observe(op("completed"));assert.deepEqual(visible,[]);preview.dispose();
});

test("visible groups sharing one cover share one cached payload and unmounting one preserves the other", async () => {
  const f=cacheFixture(), reference=row(1).artwork_ref;let notifications=0;
  const a=f.cache.watch(reference,()=>notifications++), b=f.cache.watch(reference,()=>notifications++);
  f.cache.tick();assert.equal(f.requests.length,1);f.requests[0].resolve(png);await flush();
  assert.equal(f.requests.length,1);assert.equal(f.cache.retainedBytes(),pngBytes);assert.equal(f.live.size,1);
  const url=f.cache.get(reference);a();assert.equal(f.cache.get(reference),url);assert.ok(notifications>=2);
  b();f.cache.clear();assert.equal(f.live.size,0);
});

test("an operation cannot switch between addition retention and initial accumulation", async () => {
  for (const initial of [false,true]) {
    const reads=[];let visible=[];
    const preview=createLocalImportPreview(()=>{const d=deferred();reads.push(d);return d.promise;},rows=>visible=rows);
    preview.observe(op());reads[0].resolve(sample([row(1)],initial));await flush();
    preview.observe(op());reads[1].resolve(sample([row(2)],!initial));await flush();
    assert.deepEqual(visible.map(r=>r.provisional_group_key),[row(1).provisional_group_key]);preview.dispose();
  }
});
