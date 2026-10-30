import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { singleFileInspection, fileInfoErrorKey } from "./localFileInfo.ts";

test("active inspection is shared, bounded and never cached after completion", async () => {
  let resolve; let calls = 0;
  const inspect = singleFileInspection(() => { calls++; return new Promise(r => { resolve = r; }); });
  const first = inspect("Local/a.flac");
  assert.equal(inspect("Local/a.flac"), first);
  await assert.rejects(inspect("Local/b.flac"), e => e === "busy");
  await assert.rejects(inspect("Local/a.flac", {}), e => e === "busy");
  assert.equal(calls,1); resolve({sample_rate:96000}); await first;
  const second = inspect("Local/a.flac"); assert.equal(calls,2);
  resolve({sample_rate:44100}); assert.deepEqual(await second,{sample_rate:44100});
});

test("failed requests release admission and errors are localized without raw details", async () => {
  const inspect = singleFileInspection(() => Promise.reject("permission_denied"));
  await assert.rejects(inspect("a")); await assert.rejects(inspect("b"), e => e === "permission_denied");
  assert.equal(fileInfoErrorKey("/private/location"), "unavailable");
  for (const locale of ["en","pt-BR"]) {
    const data = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url)));
    for (const code of ["invalid_local_path","outside_configured_roots","file_not_found","cue_virtual","unsupported_format","corrupt_header","permission_denied","unavailable","stale","busy"]) {
      assert.ok(data.fileInfo.errors[fileInfoErrorKey(code)]);
    }
  }
});

test("inspection is an explicit Local row action with cancellation and accessible modal", () => {
  const read = path => readFileSync(new URL(path,import.meta.url),"utf8");
  const browser = read("../components/LocalBrowserView.tsx");
  assert.equal((browser.match(/setFileInfoTrack\(\{ track:/g) || []).length,2);
  assert.match(browser,/localAlbumCatalog.getSnapshot\(\) === fileInfoTrack.catalog/);
  const modal = read("../components/LocalFileInfoModal.tsx");
  assert.match(modal,/role="dialog" aria-modal="true"/);
  assert.match(modal,/if \(active\) setState/g);
  assert.match(modal,/active = false/);
  assert.match(modal,/if \(available\) void inspectLocalFile/);
  assert.doesNotMatch(modal,/setInterval|setTimeout/);
});
