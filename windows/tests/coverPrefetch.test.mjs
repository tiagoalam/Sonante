import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const adapters=readFileSync(new URL("../src/adapters.tsx",import.meta.url),"utf8");
const shared=readFileSync(new URL("../../src/components/LocalBrowserView.tsx",import.meta.url),"utf8");
const vite=readFileSync(new URL("../vite.config.ts",import.meta.url),"utf8");
const scopes=readFileSync(new URL("../../src/utils/localAlbumPresentationCache.ts",import.meta.url),"utf8");

test("Windows restores bounded Local thumbnails before optional local-only prefetch",()=>{
 assert.match(adapters,/const WINDOWS_COVER_PREFETCH_LIMIT=100/);
 assert.match(adapters,/slice\(0,WINDOWS_COVER_PREFETCH_LIMIT\)/);
 assert.match(adapters,/WINDOWS_COVER_PREFETCH_CONCURRENCY=1/);
 assert.match(adapters,/Promise\.all\(Array\.from\(\{length:WINDOWS_COVER_PREFETCH_CONCURRENCY\},worker\)\)/);
 assert.match(adapters,/WINDOWS_COVER_PREFETCH_MAX_SOURCE_CHARS=12\*1024\*1024/);
 assert.match(adapters,/cover\.length>WINDOWS_COVER_PREFETCH_MAX_SOURCE_CHARS/);
 assert.match(adapters,/localArtworkResolver\.resolve\(album,false,/);
 assert.match(adapters,/localAlbumThumbnails\.get\(album\)/);
 assert.match(adapters,/localAlbumThumbnails\.capture\(album,image,current,"already_complete"\)/);
 assert.match(adapters,/latest\.albums===version/);
 assert.match(scopes,/folder\.startsWith\("\/"\) && !folder\.startsWith\("\/\/"\)/);
 assert.match(scopes,/\[A-Za-z\]:/);
 assert.match(shared,/!coverChecked \? localAlbumThumbnails\.get\(album\) : null/);
 assert.match(vite,/"LocalBrowserView"/);
});

test("prefetch shares the existing resolver, stays below visible reads and cannot search online",()=>{
 const resolver=readFileSync(new URL("../../src/utils/localArtwork.ts",import.meta.url),"utf8");
 assert.match(resolver,/schedule\(task: \(\) => Promise<string \| null>, background = false/);
 assert.match(resolver,/Number\(a\.background\) - Number\(b\.background\)/);
 assert.match(adapters,/localArtworkResolver\.resolve\(album,false,\(\)=>!current\(\),undefined,current,true\)/);
 assert.doesNotMatch(adapters,/getOnlineAlbumCover|getOnlineCoverCacheStatus/);
});
