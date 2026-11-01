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

test('managed startup reuses a watched original mirror proof without dropping admission boundaries',()=>{
 const initial=section('fn start_initial_mpd(', '#[cfg_attr(mobile');
 assert.match(initial,/ReadonlyMirrorSnapshot::open_for_startup\(&library_dir\)/);
 assert.match(initial,/admission\.origins=admission\.observe_origins\(\)\?/);
 assert.match(initial,/admission\.validate\(config\)\?/);
 assert.match(initial,/supervisor\.start_initial_existing\(config,&admission\)/);
 const mirror=readFileSync(new URL('../src-tauri/src/local_media.rs',import.meta.url),'utf8');
 const capture=mirror.slice(mirror.indexOf('pub(crate) fn open_for_startup('),mirror.indexOf('pub(crate) fn has_change_watch('));
 assert.match(capture,/let result=Self::capture\(library\)/);
 assert.match(capture,/Self::finish_startup\(result\?,MirrorChanges::open\)/);
 assert.ok(capture.indexOf('result.revalidate_full()?') < capture.indexOf('changes.check()?'));
 assert.match(capture,/if let Ok\(changes\)=changes \{changes.check\(\)\?;result.changes=Some\(changes\);\}/);
 const validation=section("impl InitialManagedGeneration<'_>", 'fn start_initial_mpd(');
 assert.equal((validation.match(/self\.mirror\.revalidate\(\)/g)||[]).length,2);
 for(const guard of ['self.owner.validate()', 'self.profile.validate()', 'self.library.validate()',
  'self.transaction.belongs_to', 'shutdown_requested()', 'check_startup(', 'self.profile.file_identity(',
  'self.profile.read(', 'self.observe_origins()?!=self.origins'])assert.ok(validation.includes(guard),guard);
 const supervisor=readFileSync(new URL('../src-tauri/src/supervisor.rs',import.meta.url),'utf8');
 assert.equal((supervisor.match(/admission\.validate\(cfg\)/g)||[]).length,4);
});
