import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const lib=readFileSync(new URL('../src-tauri/src/lib.rs',import.meta.url),'utf8');
const section=(start,end)=>lib.slice(lib.indexOf(start),lib.indexOf(end,lib.indexOf(start)));
test('opening an indexed library restores the queue without scheduling scans or watcher registration',()=>{
 const startup=section('async fn complete_startup_after_ready(', '\n#[tauri::command]\nasync fn play_tracks(');
 assert.match(startup,/restore_cached_queue_after_ready/);
 assert.doesNotMatch(startup,/start_local_watcher|run_managed_mirror_refresh|rescan_library|request_library_refresh|start_context/);
 const ready=section('RunEvent::Ready => {','RunEvent::ExitRequested');
 assert.match(ready,/complete_startup_after_ready/);
});
test('filesystem notifications cannot enter automatic index refresh; explicit verification stays available',()=>{
 const watcher=section('fn start_local_watcher(', 'async fn complete_startup_after_ready(');
 assert.doesNotMatch(watcher,/run_local_filesystem_batch|run_managed_mirror_refresh|refresh_existing_local_content/);
 assert.match(watcher,/Reconcile::Blocked/);
 assert.match(lib,/fn run_managed_manual_rescan[\s\S]*?ManagedMirrorRefreshMode::Manual/);
 assert.match(lib,/async fn rescan_library[\s\S]*?run_managed_manual_rescan/);
});
