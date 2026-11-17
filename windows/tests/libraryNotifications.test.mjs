import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const core = readFileSync(new URL("../src/core.ts", import.meta.url), "utf8");
const adapters = readFileSync(new URL("../src/adapters.tsx", import.meta.url), "utf8");

test("locally handled library failures do not also raise the generic global error toast", () => {
  const quiet = core.match(/const quiet=new Set\(\[([\s\S]*?)\]\);/)?.[1];
  assert.ok(quiet);
  for (const command of ["windows_configure_sources", "windows_check_library"]) {
    assert.ok(quiet.includes(`"${command}"`), `${command} remains surfaced by its local operation handler`);
  }
});

test("library status errors stay specific, compact, dismissible and inside the browser header", () => {
  assert.match(adapters, /t\(`windowsLibrary\.errors\.\$\{status\.error\}`,[\s\S]*?t\(`windowsPort\.errors\.\$\{status\.error\}`/);
  assert.doesNotMatch(adapters, /fixed bottom-52|left-\[17rem\]/);
  assert.match(adapters, /setDismissed\(noticeKey\)/);
  assert.match(adapters, /aria-label=\{t\("windowsPort\.dismiss"\)\}/);
  assert.match(adapters, /hideWindowsCatalogActivity windowsLibraryStatus=/);
  assert.match(adapters, /max-w-full.*text-\[10px\].*truncate/);
  assert.match(adapters, /status\.phase==="cached"&&verifiedScanEpoch\.current!==status\.epoch/);
  assert.match(adapters, /if\(!available\|\|restoredCache\|\|/);
  assert.match(adapters, /!status\.error\|\|status\.error==="windows_library_check_unconfirmed"/);
});

test("restoring a provisional cached catalog is silent while an explicit verification can still notify", () => {
  assert.match(adapters, /value\.phase==="scanning"&&typeof value\.epoch==="number"\)verifiedScanEpoch\.current=value\.epoch/);
  assert.match(adapters, /const restoredCache=status\.phase==="cached"&&verifiedScanEpoch\.current!==status\.epoch&&\(!status\.error\|\|status\.error==="windows_library_check_unconfirmed"\)/);
  assert.match(adapters, /metadataUnconfirmed&&status\.phase!=="cached"\?t\("windowsLibrary\.metadataWarnings"/);
  assert.match(adapters, /const cachedError=status\.phase==="cached"&&status\.error&&!metadataUnconfirmed/);
});

test("metadata decode warnings have an explicit completion message instead of the generic operation error", () => {
  assert.match(adapters, /metadataWarnings\?:number/);
  assert.match(adapters, /metadataUnconfirmed[^;]*t\("windowsLibrary\.metadataWarnings"/);
  assert.match(adapters, /const alert=!!status\.error&&!metadataUnconfirmed/);
});

test("normal cache restoration is silent and completed scan progress disappears unless warnings or an error remain",()=>{
 assert.match(adapters,/if\(!available\|\|restoredCache\|\|/);
 assert.match(adapters,/status\.phase!=="scanning"&&\(!!status\.error\|\|!!status\.warnings\)/);
 assert.match(adapters,/status\.phase==="scanning"\?t\("windowsLibrary\.scanning"/);
 assert.match(adapters,/export function LocalWatchNotice[\s\S]*?return null/);
});

