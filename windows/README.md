# Sonante — Windows 0.6.2 development

## Preview35 grouped candidate — original analyzer on the EQ route

The original Now Playing windows and event/owner/lease schema now receive real
RMS/peak/48-band data from the owned EQ consumer. The optional observer uses the
sole existing PCM reader, fixed nonblocking capture buffers and the original
Linux0.6.2 FFT/level calculation. A single lazy native collector validates source,
configuration revision, presentation lease, sequence and sample age. Startup,
paused playback, Simple mode, ordinary Shared and Direct have no analyzer feed;
no audio route is started or changed to obtain one. Metrics are pre-DSP and
include MPD software gain; they do not measure output-device or DAC levels.

Camilla candidate3 is explicitly pinned with its complete corresponding sources,
base2 plus observer patches, vendored dependencies, original notices, build
recipe and source drift gates. DSP/render/audio reader bodies, PCM ABI, Shared181
and MPD3 pins are preserved. Portable synthetic-PCM/control/event tests and a
fresh Windows cross-build passed; native Windows interaction/audio/hardware
validation is pending. Preview34 and all prior engine controls remain preserved.
Installer qualification and native acceptance are separate gates; see the supplied
private delivery receipt.

Network protocol rejection now records fixed buffer/header/provider/offline
subreasons in About/Copy diagnostics from the same existing provider response.
The acceptance policy, native call count and generic IPC error are unchanged;
this improves diagnosis and does not qualify the reported NAS failure.

## Preview33 baseline (historical)

Preview33 is the first private Windows package from the unified Linux 0.6.2
base. It connects the original Full/Simple configuration and listening-session
views/services, preserves existing beta preferences, fixes UNC display roundtrip,
adds fixed-stage/Win32 network diagnostics, and resolves the selected shared
Plex resource grant during initial authentication. It also includes the post32
EQ startup-confirmation correction. Native Windows validation of these deltas
is pending; Preview32's accepted audible EQ does not qualify the new build.

The app still has read-only metadata inspection. The separate own-fixture FLAC
qualifier is not a library writer and is not bundled or invoked by the app.
Shared/Direct/EQ runtime pins, DSP policies and original Linux sources/UI remain
unchanged. Existing Shared-to-EQ and random-step guards remain. Network folders
are still a reported native failure pending the next Windows diagnostic result.
Listening-session progress suspends conservatively after queue-generation
changes; existing queue bounds refuse oversized selections without truncation.

## Earlier preview history

Preview24 was preliminarily accepted for Local multiple roots, radio, Navidrome,
Plex, mixed queues/playlists, artwork/history, remote smart rules and source facts.
Preview25 adds the existing Linux EQ editor/preset/compare flows through Windows
adapters, and experimental Direct with a stable Windows endpoint. It also includes
the post24 Local observed-added journal; legacy dates remain unknown.

## Preview27 grouped delta — source health, gain and comparison ownership

The latest candidate adds the post26 local container-server exit signal to the
resource-free guardian. Shared gain reaches native admission directly, without a
status query ahead of the gesture. Busy/shutdown and context tickets refuse gain
queued behind a changed audio route, while ordinary status polling and consecutive
gain gestures preserve the context. Mute retains the displayed owner token.
Gestures invalidate the comparison epoch before waiting; Status and comparison
commands themselves do not. The comparison DSP policy remains byte-exact to Linux.

Continuity faults stay recorded in LOST/STOP. Cleanup acknowledgement separately
requires signalling and confirmed termination of both exact owned Jobs; handled
peer/probe faults no longer masquerade as a second cleanup failure. Both peers
are always attempted. These changes add no new engine, PCM ABI, runtime pin,
master-volume synchronization or Linux/shared-UI change. Native and hardware
validation remain pending. Sealed26 and both optional qualification tools stay
preserved; SessionQualification1 is the earlier exact78c13d9 test checkpoint.

## Preview26 delta — original first-use wizard and EQ lifecycle

The Windows adapter now returns the native first-use flag to the original
WelcomeWizard. A fresh profile persists a pending folders/sources record before
account stores initialize. Interrupted setup resumes; old profiles stay out of
the wizard without changing their records. Corrupt/linked/deleted/externally
changed current settings are preserved and rejected. Source/audio/account saves
must succeed before the final setup flag is committed; indexing is progressive
and is not a completion gate. The original Direct equipment-volume confirmation
and new PIN flow remain, while the completed wizard reads back native config
instead of publishing draft credentials or guessed saved state to App.

Own record fingerprints advance on each durable publication, allowing rollback
of that exact publication if a later scan/host step fails. This is optimistic
validation under the existing single-profile/store lock, not a filesystem CAS
against a malicious external editor. Startup still creates no engine/listener or
autoplay. Synthetic fixture and cross-build checks do not replace Windows UI tests.
Preview26 groups this wizard with independent EQ source/stdio/MPD-status
supervision, queue-publication rollback, and terminal radio/sleep controls.
Guardian source probes hold atomic flags only, never files/accounts/listeners;
queue insert/remove publishes its watch before releasing leases. A failed new
candidate preserves a healthy old route. Uncertain queue/config state retires
only owned sessions, keeping the confirmed logical journal.

Playing EQ seek/track cuts retain the old generation through durable queue
publication. A failed write terminates the candidate before resuming an old
Playing source with PCM Pause0; newer intents never cause rollback autoplay.
Radio Stop validates the station and uses the existing generation action.
EQ sleep termination captures the logical queue then closes both owned Jobs,
even when capture fails, without starting replacement engines/listeners. It
leaves a stopped logical queue and inactive EQ; apply a preset again before the
next playback if desired. Native Windows and hardware qualification are pending.

## Post26 source checkpoint — native Session qualification

The already sealed Preview26 remains unchanged. The following source checkpoint
adds a container-server exit flag to resource-free source supervision, plus an
explicit ignored native integration fixture using the actual Session/Job/stdio/
ring/queue/journal modules with a test-executable-only File sink. It cannot select
a File sink in a release app and does not open audio endpoints or user profiles.
See [the qualification contract](qualification/session/README.md). Native execution
and hardware remain pending; a successful cross-build is not native evidence.

## Preview25 audio scope — native qualification pending

Approved WASAPI Shared playback still uses its original separately pinned MPD.
Direct and EQ explicitly use full-codec MPD candidate2. EQ uses CamillaDSP4.1.3
with a Windows-only private PCM input and corrected WASAPI render I/O. Source,
patches, verified vendored dependencies, unchanged DSP/conversion/Cargo profiles,
licenses and rebuild recipes accompany both candidate runtimes. Previous candidate
files and installers remain preserved. Cross-build, portable contract tests and
source audits do not prove native Windows IPC, WASAPI behavior or physical output.

Startup restores a logical paused queue without an engine, listener, autoplay or
active EQ, even when a preset was selected previously. For this initial native
EQ test, reopen Sonante and apply a preset BEFORE starting playback. Switching
from an existing Shared Playing/Paused session is explicitly refused because its
approved upstream WASAPI drain has no owned rendered-cursor attestation. Route
changes stay paused; live preset and original level-matched comparison use the
same owned generation. Seek/stop/jump prepare a fresh generation; ordinary pause
retains the FIFO. Next/Previous with EQ+random are explicitly refused until MPD
play-order snapshot/restore is implemented. Natural random runs within its engine;
recreation does not yet preserve that internal history. No silent order fallback.

Internal EQ PCM is48kHz/S32/stereo. Shared WASAPI rendering can convert to its
supported shared format; this is not native DSD or bit-perfect. Source facts,
internal PCM, engine negotiation and physical output remain distinct. Direct
requires the selected stable IMMDeviceID and exact supported source format, no
sample-rate/channel/precision downgrade; only reversible integer widening is
allowed. Equipment-volume confirmation/mute use the original policy. Direct has
no EQ, no DoP, and no inferred physical output attestation. Busy/format/hotplug,
DAC behavior and all new native paths require Windows/hardware validation.

A separate File-only synthetic PCM qualification utility is provided for return
of the maintainer. It opens no audio endpoint and reads no app profile/user media;
it must be run explicitly. Its compilation is not a successful native test.

## Historical preview notes (superseded by Preview25 above)

## Preview22 current scope (supersedes historical sections below)

The original App, SettingsModal, Plex browser/search/discovery/artists, favorites,
queue and playlists now use Windows adapters. Saved Plex credentials stay in the
CurrentUser DPAPI backend; the renderer receives only account presence, selected
server and a public binding generation. New PIN/manual credentials follow the
original login flow. Plex.tv authentication opens only the official HTTPS page.
The Windows build maps original credential-presence/cache checks to public account
state without changing Linux source bytes or creating another settings/player UI.

Local, Navidrome and Plex stable references can share an owned MPD queue and
manual playlist. Playback preparation proves the current remote account/reference;
private loopback capabilities reach MPD without returning tokens or authenticated
URLs to the renderer. Every distinct account fence covers the final MPD effect.
Startup restores only paused logical state; no MPD, playback listener or autoplay.
Changing an account/source checkpoints any affected queue before retiring leases.

Roots/flags use library-v3 with absence-only read migration from v2/v1. Queue-v3
and existing playlists are retained. Favorites-v2 preserves the old v1 bytes,
scopes Plex records/images by server, and never deletes orphan references. Corrupt
current data is an explicit error, never permission to reset or import a backup.

Native Plex PIN login/TLS, catalog/covers/playback/seek/transitions, mixed queue
reorder/remove/reopen, playlists and DPAPI restore remain pending human validation.
Plex queue artwork is not persisted in the logical journal and may be absent;
remote source facts/history and smart catalog/edition analysis remain unavailable.
Output is still the default WASAPI Shared device with software volume. Device
selection, exclusive output, PCM/CamillaDSP, DoP and DSD are not completed.

The following sections preserve the history of earlier previews; older capability
and path limitations are superseded by this Preview22 scope.

This build mounts the existing src/App.tsx with the existing LocalBrowserView,
PlayerBar, QueueDrawer, search, artists/years/genres, favorites and preferences.
Windows-only IPC and capability adapters connect those views to the managed
Windows MPD/catalog backend. There is no separately maintained Windows player UI.
Original Linux sources and behavior remain unchanged. The old pilot screens
have been removed from the source; previous installer/source deliveries remain.

## Current scope

- Up to32 chosen, non-overlapping local music folders, read-only tags/covers/catalog, progressive
  indexing, album/track playback and an owned native MPD queue.
- MPD 0.24.18, default Windows WASAPI Shared output, software volume. No claim
  of bit-perfect or exclusive output. The legacy config enum `pipewire` is mapped
  to the shared route in the Windows adapter; Windows does not run PipeWire.
- The local queue, current index, position and volume are saved atomically under
  app-local queue-v2/queue.json (absence-only read migration from v1). Normal window close reads a final status before
  terminating the child. Checkpoints occur on operations/cursor changes and at
  most every five seconds during status polling. Abrupt termination can lose
  progress since the last successful checkpoint.
- Reopening restores only logical paused state; it does not start MPD, create
  a playback-media server or open audio. Read-only catalog/artwork work is separate. Explicit Play revalidates/locks the media and prepares
  MPD. Missing media or rejected restoration keeps the logical queue intact.
- Clear Queue explicitly closes the session and clears the saved queue. An
  unavailable engine has an explicit Recover Queue action that closes the owned
  engine and preserves the last confirmed queue, without automatic playback.
- Search uses the original shared UI. Native track search tolerates accents and
  case; album presentation and ordering use the existing frontend controls.
  Preview 9 migrates favorites and artwork progress to native Windows stores;
  the old WebView values remain untouched as backups.
  Favorite covers are resolved from the current native catalog, including
  favorites saved by Preview 4, with two concurrent reads and no large images
  written into preferences. A failed cover preserves favorite metadata and
  reports an actionable warning. Cold requests now wait within the same scan epoch
  for the requested album (up to30seconds), without waiting for the entire catalog.
- Repeat off/all/one and random use the original playback_options.rs state
  machine without modifying it. Only MPD-confirmed options are displayed;
  stale requests and rejected/unconfirmed changes fail explicitly. Options are
  cached with the queue and reapplied to a prepared engine before activation.
  Legacy queue caches remain readable. On silent reopen, controls stay disabled
  until explicit Play starts MPD. Random traversal history/order is not saved.
  Next/previous while active follow MPD random/repeat semantics; reaching the end
  with repeat off stops rather than restarting the last track.
- The original Radio Browser UI now connects to native directory/favorites/manual
  station and live playback adapters. Radio is transient and does not resume on
  startup; native acceptance is pending. Plex/Navidrome, tag editing, advanced
  audio diagnostics, online artwork, PCM analyzers, CamillaDSP and exclusive output are not ported in this
  preview. Navigation/options are disabled where adapted; invoking another
  unsupported operation returns an explicit unavailable error, not fake success.
  Setup goes directly to the shared main UI; choose a folder in Preferences →
  Sources. The original Linux first-run wizard is not part of this preview.

## Queue editing in Preview 6

The original queue/context/selection controls now map to Windows-native enqueue
(next/end), move, remove and move-next operations. This delta is included in Preview 6 and still needs native Windows validation. Active
edits use addid/moveid/deleteid without stop/play/clear/restarting the process.
Exact MPD occurrence IDs preserve duplicate tracks and the paused cursor/time.
A playing or paused current track cannot be removed. Captured queue revisions
reject outdated drag/drop edits. With no engine, edits write the logical queue
atomically before publication and never start audio or open media.

Pre/post confirmation reads playlistinfo in pages of 16 and checks version,
positions, IDs, paths and cursor. A partial response/mutation blocks further
edits/checkpoints and preserves the last confirmed cache until Recover Queue;
there is no blind clear/replay/retry. Confirmed live edits that fail persistence
report an error; inspect the queue before repeating. Logical write failures
preserve the old in-memory and on-disk queue. Media locks follow occurrences,
including reorder/removal, and remain held through any uncertain operation.
Insertion is limited to the active queue's local root and 1000 total entries;
indexing/mutation and read-proof work have bounded budgets. Cross-root insertion
is explicitly unavailable until the multiple-folder backend is ported.

## Manual playlists in Preview 6

The original src-tauri/src/playlists.rs store is compiled unchanged on Windows,
with Windows-only schema/validator and atomic-persistence adapters. The original
PlaylistsView and PlaylistPickerModal handle create/rename/delete, batch addition,
duplicate occurrences, removal, reordering, play from an item and ephemeral
shuffle. Local smart playlists are adapted as described under Preview 11. Only local catalog
references can be added by Windows commands; remote sources remain unported.

Playlist files use the original v1 schema under app-local user-data/playlists.json.
A serial worker and per-operation cross-instance file lock protect read/modify/
write; successful writes use synced random temporary files and atomic replacement.
Malformed or future-version files are preserved. Limits: 1000 items per playlist,
16 MiB stored file, and bounded metadata/requests. Worker shutdown rejects new
requests and drains with an explicit timeout. It does not hold a store lock during
catalog/media resolution or playback startup.

Opening a playlist checks presence without starting MPD. Missing and temporarily
unavailable remain distinct and never remove entries. Explicit Play reloads the
stored playlist, revalidates references and intent, prepares the owned MPD route,
and reports skipped entries. An unavailable selected item refuses playback rather
than silently starting somewhere else. The shuffle option does not reorder stored
items. UI QA uses a separate synthetic transport; native Windows validation is
pending for this delta and the queue-editing checkpoint.

## Session sleep timer in Preview 7

The original SleepTimerControl, service and frontend store are reused unchanged.
A Windows worker owns a monotonic 15/30/60-minute deadline independently of WebView
visibility. Timer state is session-only and is not restored on reopen. Cancel and
replacement are serialized with any already-started STOP; they cannot acknowledge
before that effect returns. Expiry admits a playback intent before waiting on the
MPD worker, superseding pending source preparation. An operation already past its
final admission check may finish before the queued STOP runs.

The timer sends STOP once and confirms the stopped state without clearing or
rebuilding the queue. An already-quiet logical queue never starts MPD or opens
media. On rejected/unconfirmed STOP, only the proven owned engine is terminated;
the last confirmed queue cache is retained and the timer reports failure. No
automatic replay or repeated stop attempts occur. Shutdown invalidates the timer
and drains independent workers. A full native 15-minute expiry test is pending.

## Original expanded player in Preview 7

The shared NowPlayingHost, presentation, layouts, queue, fullscreen controller,
detach/reincorporate handshake and window service remain unchanged. The Windows
adapter provides native presentation-host generation/session/lease checks: main
is the only transfer authority, stale cleanup cannot stop the new host, and PCM
availability is explicitly false. VU/spectrum do not emit fabricated frames. Their
placeholder states explain that Windows PCM capture has not been ported.

Seek captures the rendered generation and queue occurrence before pointer or
keyboard interaction, including in the detached window. Host transitions never
send playback/queue commands. Closing main requests complete application shutdown
even with a detached window open; closing only the detached window preserves the
audio session. WASAPI route facts use the original signal dialog and MPD format
parser without showing Linux ALSA output claims. MPD-reported format is not proof
of the physical Windows output or what the DAC receives.

## Native validation

Preview16 has an initial normal-use acceptance; the grouped exceptional scenarios remain unverified.
Close the installed preview and install Preview 20, which includes Previews 6–19.
One representative native pass remains for the grouped changes:

1. While playing, append an album and move/remove future queue entries without
   restarting the current track. Save/reorder a manual playlist, close/reopen and
   play it; queue/playlist order must persist with no automatic startup audio.
2. Open the expanded player, change layouts, detach/reincorporate, and use
   pause/next/seek/fullscreen. Close main while detached: audio must stop and both
   windows close. Closing only the detached window must leave playback running.
3. Check existing Favorites covers, remove one and add another; close/reopen.
   Removal must persist without reimporting the old backup. Local album covers
   must still load and their progress remains native metadata only.
4. Inspect a local file using its existing info button. Play, pause, advance
   between different formats, close/reopen and Play the restored occurrence.
   Source badges must follow the current file and not show Hi-Res for MP3.
5. Play two local tracks and open Recentes. Pause/seek must not duplicate entries;
   disable recording, advance, enable again, then advance once more. Only new
   plays while enabled should be recorded. Clear with the original confirmation:
   the current track must not immediately reappear. Reopen silently and play a
   saved entry/open its album using the original controls.
6. During playback open signal details: check the observed output name and time
   against the MPD audio session in Windows. Refresh after a device/default
   change; do not assume the MPD follows a new default. Paused/restored/error
   sessions must not display a live observation. No DAC-format/fidelity claim.
7. Arm/cancel a timer, then let a 15-minute timer expire during playback. It must
   stop while retaining the queue; restarting the app must not restore the timer.

Portable tests, cross compilation and browser fixtures are distinct from native
Windows validation. The fixture uses synthetic IPC, window events and transport,
not real audio or native window creation. Do not repeat accepted themes/languages
routinely. Remote services and PCM analysis remain unavailable. Local smart playlists are included as described below.

## Original file inspection and source badges in Preview 8

The existing LocalFileInfoModal now maps to a Windows-native read-only adapter in
Preview 8. The preserved Preview 7 installer does not include this delta. All six original
header-reader modules compile unchanged, including their existing tests. Format,
codec, compression, depth/rate/channel and duration facts retain the original
parser limits and never derive fidelity from a filename or the MPD output.

Only an opaque native catalog URI is accepted. Inspection requires a ready
catalog and revalidates its root/epoch/membership. Native handles check the final
path and 128-bit file identity, reject UNC/network drives, reparse points and
cloud placeholders, hold ancestors through the read and deny normal write/delete
sharing. File identity/metadata and paths are checked again before publication;
unsupported identity/filesystem queries fail explicitly. These Windows sharing
and filesystem behaviors still need native validation.

The same global inspection permit stays with the blocking worker until it ends,
even if the dialog closes. Header readers have their own structural limits plus
an outer 8 MiB/32768-I/O-operation budget and a two-second deadline checked between
I/O calls; this does not interrupt an already-blocked filesystem call. No tag
writes, decoding, transport changes, persistent result cache or MPD startup occur.
Errors are displayed by the original file-info dialog. Source badges for the
current occurrence use the same original readers, SourceAudioBadges and acquisition
hook. A read-only binding published by the existing MPD worker checks generation,
intent, cursor, opaque URI and catalog/queue physical path before and after a
read. It clears on failure/shutdown. No second MPD poll is introduced.

The Windows hook adapter adds observed native generation/live state to the
original context. Logical restored queues do not read source facts until Play;
that transition reacquires even with the same URI/index. Stop/Recover/replacement
invalidate, while elapsed/volume/pause telemetry does not trigger new reads.
Both player hosts receive the same original atomic source-fact snapshot. These
are encoded-source facts, not output/DAC or bit-perfect evidence. A busy permit
or unsupported/stale file yields no badge; there is no speculative classification.

## Native favorites in Preview 9

The original local favorite cards, FavoritesView and favoritesService are
unchanged. A Windows store now preserves the existing id+source toggle identity,
order and flexible year field in user-data/windows-favorites-v1.json. The local
card's ID is its opaque folder_path; matching legacy hash IDs remain readable.
The original favorites.rs orchestration depends on AppConfig/Plex/POSIX local
resolution, so the Windows adapter retains its public schema and semantics
without importing fake platform stubs. No media, queue or MPD operations occur.

First access checks the native store. Only an absent destination permits an
atomic import of the old sonante_windows_favorites value; later/stale imports
return native data and cannot resurrect a removed album. The old WebView value
is left untouched as a backup. After migration, native storage is authoritative
and a broken WebView getter is irrelevant. A malformed, future-version or unsafe
file fails explicitly and is preserved; it never becomes an empty list or a
silent reimport. Each operation holds the existing Windows store lock through
read/modify/atomic replacement, including between app instances. Busy is explicit
and no toggle is retried automatically. At most 16 operations are admitted;
shutdown stops admission and waits up to three seconds outside the UI/audio
workers, logging failure if a filesystem call remains blocked. Atomic replacement
protects the previous/new document on abrupt exit; it cannot cancel an OS call.
Limits: 10000 albums/4 MiB, bounded fields,
local opaque identities only. Artwork is excluded from disk and hydrated using
the existing two-read bridge. The main UI receives success only after saving.

This delta is included in Preview 9; the preserved Preview 8 does not include it. Windows-native
migration/sharing/replace behavior still needs a representative test. Downgrading
to Preview 8 reads the untouched old WebView backup; it does not see edits made
only in the new native store. Keep the native file when testing a downgrade.

Multiple local roots remain unavailable: the current queue/session cache has one
music_root. The pinned MPD rejects local files outside music_directory on Windows
(Client::AllowFile), and its local storage plugin has no create_uri mount factory.
A cross-volume namespace needs a separate backend plan; merely enabling the
original multiple-folder UI would leave playback/queue behavior incomplete.

## Native artwork progress in Preview 9

The original LocalArtworkEnrichment, resolver and progress service are unchanged.
The Windows adapter now stores only progress metadata in
user-data/windows-artwork-progress-v1.json, with the original result enum,
online_complete and Unix-second checked_at fields. It does not store images or
provide an online artwork cache. As in the original pure validation policy,
results online/none cannot retain online_complete without a corresponding valid
positive/negative cache file; Windows has no such cache, so those flags are false
in the read view. Local/embedded/ineligible semantics remain unchanged.

The first access atomically imports the old sonante_windows_artwork only when
no destination exists. Preview8 Date.now() milliseconds are normalized to Unix
seconds during import; later writes use the backend clock. The WebView backup
is untouched. Native data is then authoritative, including after stale import.
Invalid/future/unsafe files are preserved and rejected explicitly; unlike Linux's
disposable-cache recovery, Windows does not silently overwrite these files.
Batches retain the original 256 limit (the original UI flushes at 200); all
entries validate before write. Limits: 100000 IDs/4 MiB, opaque catalog hashes.
A serial worker queue of 16, cross-instance file lock and existing atomic Windows
writer protect merges. Shutdown stops admission and drains at most three seconds
alongside other workers; it cannot cancel an already-blocked filesystem call.
There is no media/MPD/audio I/O and no second presentation/UI implementation.

This delta is grouped with native favorites in Preview 9.
Downgrading to Preview8 reads the old WebView progress backup and cannot observe
later native changes. Native Windows sharing/replace/migration still needs test.

## Build on Windows x64

Prerequisites: Node compatible with locked Vite dependencies (20.19+ or 22.12+),
Rust x64 MSVC, Visual Studio Build Tools with C++/Windows SDK and WebView2.

```powershell
cd C:\path\to\sonante
powershell -NoProfile -ExecutionPolicy Bypass -File .\windows\Build-Preview.ps1
```

The helper fetches pinned official MPD binary/source and both detached signatures,
verifies all four SHA256 hashes and the binary's internal version, installs locked
npm dependencies and builds NSIS. The runtime resource map lists these four files
explicitly: unrelated cache files are preserved but cannot enter the package via
the previous runtime-directory mapping. Manifest URLs/hashes/version and the four
destinations must agree before download or version-query execution. Changing the
MPD version also requires updating the explicit resource filenames.

Signature hashes are reviewed pins, not a new GPG verification by this helper.
The cross-build pins were checked against the upstream key fingerprint recorded
in mpd-runtime.json; this script does not install GPG or fetch/import a key. Exact
source and licenses remain bundled. Last cross-build toolchain was Rust1.99.0;
a native Windows build with these helpers still needs acceptance.
`-SkipNpmCi` reuses
installed dependencies. No global policy, service or firewall changes. The
unsigned installer runs for the current user; WebView2 bootstrap needs internet
only if its runtime is missing. MPD binary, exact upstream source and licenses
are bundled. Full static dependency license/source audit is required before any
public distribution; this is a private test build.

## Backend limits and ownership

Local drives only; UNC/network roots and reparse points/symlinks are excluded.
Use files downloaded to disk, not cloud placeholders. Supported catalog extensions:
FLAC, WAV, MP3, OGG, OPUS, M4A, AAC, AIF/AIFF and WV; MPD determines decoding.
Tags have an 8 MiB read budget; filename fallback warns on unreadable metadata.
JPEG/PNG cover/folder/front or embedded pictures are capped at 4 MiB. Catalog:
100000 tracks, 500000 entries, 64 levels. Queue: 1000 tracks, 20-second indexing
budget; large queues can time out while the previous source continues.

Queue durations are unknown until MPD observes a track; the UI does not invent
album/queue totals. Each remaining queued occurrence and the verified executable stay read-locked.
Removed occurrences release their media handle only after confirmed editing;
uncertain edits retain locks until recovery. Close the session before modifying
files that remain queued.
Children are created suspended, assigned to an owned kill-on-close job and then
resumed, with explicit NUL/stdout/stderr handles and restricted inheritance.
Authenticated IPv4 loopback checks the connected tuple/child before credentials.
Candidate-first replacement/rollback remains. Requests admitted by the shared UI
are checked before activation; seek retains its captured cursor across a drag.

The folder, queue cache and session config/log contain local paths; sessions also
contain a temporary loopback password. They stay in preview app-local data and
must not be published as diagnostics. Runtime folders are normally removed at
close; a crash can leave them while the kernel job still terminates the child.
`windows/runtime`, `dist`, `artifacts` are generated/ignored. No code push, CI or
public publication is implied by building this preview.

## Read-only output observation in Preview 10

The original SignalPathInfo dialog now requests a point-in-time observation of
the owned MPD process audio session. The process witness duplicates the exact
owned child handle; no PID lookup or process control is introduced. The existing
MPD worker publishes a proof only after a clean live playing snapshot, with
intent/generation/revision/process-lifetime checks around the native query.

An off-UI MTA inspection enumerates active render endpoint sessions and accepts
only an active single-process session with exact S_OK from GetProcessId. A
multi-process success, multiple candidates, removal, stale context, unavailable
API or timeout produces no device claim. It never substitutes the current
Windows default for the MPD session. The enumerator may miss a newly created
session; lack of observation does not prove lack of an output.

Only name and observation time reach the UI, on dialog open/context change or
manual refresh. It is a dated observation, not an automatic routing monitor,
device selector, DAC format measurement or bit-perfect certificate. Session
proofs and React context identities reject late results and pause/resume ABA.
A global permit bounds native work to one query; UI5s and native2s budgets do
not cancel an already-blocked system RPC. The permit remains until completion;
MPD shutdown and its Job remain independent. Native COM/hotplug validation is
pending. Shared default audio configuration is unchanged.

## Local listening history in Preview 10

Recentes reuses the original HistoryView/historyService, play/album navigation,
queue menu, recording checkbox and clear confirmation. A Windows store adapts
the original local-history contract without importing POSIX/provider/scrobbling
orchestration using fake services. It stores only local catalog references
under user-data/windows-listening-history-v1.json (v1,100entries,1MiB), with
recording enabled by default as on Linux. Direct-file/remote references are not
recorded. No physical paths, cover data, credentials or playback authority are
accepted from the saved file. Plays are revalidated by the existing catalog.

The same local identity, bounded/clean metadata, Unix-millisecond timestamps,
play deduplication, stop rearm and clear/disable semantics are characterized
against exact extracted original algorithms on pre-admitted local metadata.
Windows opaque-URI validation replaces Linux extension/CUE admission. This is
not a test of the full Linux module. The Linux code remains unchanged.

Only transitions from existing confirmed clean MPD snapshots reach a separate
serial store worker; the MPD worker never waits on history I/O. Queries and
configuration share its order. Fresh reads and the existing cross-instance lock
and atomic replacement protect data. Corrupt/future/unsafe files are preserved.
A failed write or saturated observation queue makes history explicitly
unavailable until reopen, without interrupting playback or retrying each tick.
Call4s/shutdown3s limits cannot cancel blocked OS I/O; abrupt exit can omit queued
observations. Shutdown drains independently of MPD. Native Windows sharing and
replacement remain pending; the native sharing test was compiled, not run.


## Local smart playlists in Preview 11

The original SmartPlaylists/Dialog/service, genre families, preview assembly,
round-robin distribution, cancellation, playback intents and Save Selection
flows are reused unchanged. Windows adapts only local source/catalog admission
and native rule storage. A wrapper checks the complete catalog epoch/revision
before and after original asynchronous track expansion, refusing stale results.
Explicit Play still revalidates opaque catalog URIs and physical file handles.

The native user-data/windows-smart-playlists-v1.json stores schema1 rules only
(max200 definitions/1MiB), not materialized queues or media paths. The original
pure validation/CRUD/selection bodies are characterized byte-for-byte; extracting
a shared pure module is deferred to avoid changing Linux. A bounded serial
worker, fresh reads under per-operation file locks and atomic replacement retain
invalid/future files with errors. Call4s/shutdown3s cannot cancel blocked I/O;
a timed-out mutation may finish, so reread before retrying. Favorites reuse the
native store with absent-only legacy import and no cover reads.

The Windows catalog has no added dates. They remain null; the original date
filter excludes unknowns and stable sort uses artist/title tie-breakers. The
always-visible Windows rule hint says date-filter templates will find no albums.
Folder modification time is never substituted. Local filters, track limits,
varied/album order, play/shuffle, CRUD and freezing a visible manual selection are
available for native testing. Canonical disk0 is accepted in manual playlists,
matching the actual catalog; malformed/negative/overflow disc strings are not.

## One participating process per profile in Preview 11

Before product stores, catalog or MPD workers start, an owned nonblocking file
lock admits the app-local profile. The empty windows-profile.lock is not a PID
file and is deliberately retained on disk after exit. A static handle keeps the
lock through managed-state teardown and store timeouts until process exit. No
process lookup, termination, focus forwarding or local server is introduced.
Windows opens a nontruncated, nonreparse/nonoffline regular file, denies delete/
rename while held, and does not inherit the handle. Existing MPD Job/stdIO/
restricted-inheritance/loopback ownership code is unchanged. Auxiliary windows
belong to the same process and remain supported.

A competing participant is refused before data admission with an actionable
native message. Unavailable/unsafe storage is a distinct error, never a reset.
Startup failures do not enter a partial app or expose raw paths. The popup uses
Portuguese for a Portuguese Windows UI primary language, English otherwise;
WebView-only app language preferences are not read before bootstrap. Native
second-launch/rename/handle/crash acceptance is still pending. OS lock release
may take time after termination. This protocol excludes only participating
versions: close Preview10 and earlier before running11. Older versions can still
bypass a lock they do not know about.


## Catalog durations and bounded engine admission in Preview 12

The original local album, Favorites and directory views now receive optional
duration metadata in seconds from the Windows catalog. The existing Lofty0.25.4
reader obtains properties without covers, under the Windows header reader's
8MiB/32768-I/O-call/2s-between-I/O budget. A property error falls back to the exact
previous tags-only reader, preserving accepted metadata and warning policy.
Unknown or zero duration is omitted. Lofty's default16MiB per-allocation limit
is unchanged. Duration may be estimated depending on the format: this is
presentation metadata, not playback timing or evidence of the output format.
Record.durations is memory-only; Track, Selection, NativeStatus, playback timing,
queue cache and persisted data schemas are unchanged. Catalog queries perform
no new file reads. The scan adds bounded property work, but a blocked OS call is
not cancellable; native Windows large-library performance remains unverified.

The MPD worker now admits at most32 ordinary pending/in-flight commands, with
separate reserved slots for the existing timer STOP and a single shutdown.
Atomic admission and worker-owned leases retain the bound through completion
and ACK, even if the reply receiver disappears. Channel sends never wait for a
slot. Busy admission is an error before enqueue, without retry or a synthetic
ACK. Busy status polling has no confirmed playback snapshot and reports a wait
notice; it does not request recovery of an otherwise unobserved engine. The next
original poll can confirm it. All worker operation bodies, ordering of accepted
messages, existing stopping fences, queue transactions and process ownership are
unchanged. Already admitted work can finish or be rejected during shutdown;
the existing6s shutdown deadline cannot cancel blocked native I/O. Repeated
shutdown requests are refused, not reported as completed. The timer's single
worker retains its own STOP reservation when ordinary admission is saturated.
Admission bounds MPD worker messages, not every upstream IPC preparation.

Portable comparisons against the preceding Windows catalog and synthetic
original-UI QA passed. Native Windows audio, file sharing, shutdown/stress and
large-library performance acceptance remain pending. Radio was inspected but
not enabled: its transient live-source/credential/journal contract is distinct
from local files and requires a complete source adapter rather than URL paths.


## Windows system media controls in Preview 13

An optional desktop SMTC adapter binds the current process's main HWND, using
Windows WinRT on a dedicated owner thread. Play/Pause/Next/Previous reuse the
existing Windows session commands and intent/generation fences, matching the
original Linux MPRIS basic-control targets. No separate player UI is added.
Title, artist and album are bounded informational snapshots from the existing
MPD worker; no added poll, file read or playback command occurs on publication.
A restored logical queue can appear paused; only an explicit control starts the
existing playback route. No cover, seek, Stop, timeline or volume is advertised.

Callbacks admit intents before queueing, after checking availability/capacity.
At most eight controls wait plus one executing; delayed controls cannot override
a newer UI intent. A latest-snapshot mailbox coalesces informational updates.
Native WinRT calls and MPD requests never hold the mailbox mutex. Readiness is
fenced and pending callbacks cleared before potentially blocked unregister/clear
calls. Exit drains this optional owner with a3s deadline concurrently with the
existing MPD shutdown; blocked native I/O is not cancellable. Failure disables
the optional observation and reports through the existing Windows error notice;
it does not claim a confirmed stopped engine or stop unrelated applications.

Portable concurrency/control-policy checks and Windows compilation passed.
Native SMTC activation, key routing/focus, lock-screen/panel appearance, removal
and coexistence with the owned MPD child remain unverified on Windows. No dummy
audio, loopback capture or new audio route is used to force native visibility.

This preview also includes the native-build preparation checkpoint: four pinned
MPD resources are explicitly mapped (binary/source and both signatures), so an
extra file in the runtime cache is not bundled. Prepare-Runtime validates their
hashes and resource destinations without deleting other cache contents. This
hash check does not replace the recorded signature verification. PowerShell7
Linux fixtures and the real Tauri resource resolver passed; native PS5/VS/SDK
build execution still needs testing. The window title is independent of preview
number; this build's banner and status identify Preview13.


## Confirmed library checks and independent status observations in Preview 14

The shared Check Changes action now waits for the Windows catalog's completed
scan and compares its confirmed metadata, rather than returning true when an
asynchronous scan merely starts. The native adapter validates source/expected
folder before admission, rejects concurrent checks or an already-scanning
catalog, and returns no success for stale scope, partial/failed/warned scans,
shutdown or timeout. Rescan also waits for completion. This retains Windows'
existing full read-only scan: the Linux incremental planner is not ported here.
The existing progressive catalog can clear/re-publish during that scan.

A memory-only change signature consumes already inspected path/length/mtime and
parsed album/track/search/duration metadata. Serialization streams into the hash
without a JSON output buffer or additional media read. Sorted digests are bounded
at500000 entries (Vec capacity up to16MiB); the final32bytes are not persisted.
This is a change hint, not a full content hash, filesystem atomic snapshot or
media authorization proof. Raw audio/cover bytes altered with length/mtime and
parsed metadata unchanged may be undetected. Parsed tag changes with restored
length/mtime are covered. File identity/playback admission remains unchanged.

A revisioned condition signal has its own short gate, so close does not wait
behind catalog projection work and a publication between read and wait is not
lost. The30s confirmation wait starts after admission/begin/canonicalization;
blocked earlier OS I/O is not cancelled or bounded by that deadline. Timeout
leaves the existing scan running, and another check is refused while scanning.
There is no automatic retry, MPD request, playback or media write from a check.
Native large-library timing and Windows filesystem/close behavior need testing.

The original paired status requests remain awaited on the same cadence, but
catalog-query failure no longer discards confirmed MPD playback or marks its
engine dead. A missing library observation is displayed explicitly, without
inventing a scan phase/count; native MPD failures and busy handling remain
independent. Recovery refreshes catalog presentation once even if the revision
is unchanged. Late pairs cannot overwrite newer published observations, each
caller returns its own result, and no cache is presented as a new confirmation.
No additional timer, polling, background retry or cancellation path is added;
a blocked query can still delay the paired request.

Portable250Rust cases, original catalog comparisons, bridge/notice failure and
ordering tests, unchanged original UI policy tests and TypeScript passed. These
checks and synthetic UI QA do not establish native Windows acceptance. SMTC and
all earlier unaccepted deltas remain in the manual guide, with Preview5 still
the last maintainer-accepted Windows build.


## Shared mute and queue persistence in Preview 15

The Windows worker now uses the original Linux shared_mute.rs policy unchanged.
Its single session-only memory serves the main player, detached player and mute
shortcuts. Requests carry desired mute state and context, read one generation in
the bridge and revalidate it in the owner. The worker reads actual MPD software
volume before planning. Duplicate mute/unmute requests are idempotent; an unmute
of an already audible session leaves its actual volume alone. A pending older
mute/slider request in one WebView cannot overwrite a newer one after late status.
Independent WebViews share native FIFO/policy, not a global last-click clock.

There is no guessed restoration level or persisted mute flag. The old WebView
value is left intact as a backup, but does not select a gain. As on Linux, a new
owned engine, recovery or process restart loses mute restoration memory. Queue
revisions keep the same volume owner and preserve the level across queue edits. If the
confirmed current volume is zero and no positive level is known in this context,
unmute reports that fact and asks for a manual volume adjustment. No new startup
playback, media read, audio route, timer or background retry is introduced.

A restored logical queue now saves slider/mute/Stop changes atomically before
publishing them; save failure retains its prior position, volume and queue. An
already confirmed last-occurrence deletion from a stopped live MPD queue has
different semantics: the engine effect already happened, so RAM remains empty
with an explicit persistence error/dirty state if saving fails. The old durable
file is preserved until saving succeeds, and a subsequent flush only writes the
confirmed state without replaying delete. If owned engine cleanup fails, its
object/handles stay available to existing recovery and the failure is explicit.
A cleanup failure is never reported as proof of termination.

An abrupt exit before a failed save can still restore the old durable queue,
paused. Existing explicit recovery/close/edit flush paths can retry persistence;
no added polling or automatic queue mutation is used. All existing MPD commands,
occurrence/cursor/version proof, Job ownership, stdio, loopback authentication,
cache schema, source locks and other operation bodies remain unchanged.

265 portable tests, comparisons with the unchanged Linux policy and former
failure paths, 7 bridge concurrency/context tests, 12 original player UI tests,
TypeScript and MSVC --tests passed. The two unchanged isolated child helpers were
already tested in Preview 11. These are simulations/cross compilation; native
Windows mute across hosts, saved queue/sharing/cleanup behavior and actual sound
remain pending. Preview 5 is still the last maintainer-accepted Windows delta.


## Ordered status observations and held cover reads in Preview 16

The existing Windows worker attaches a lossless decimal-string observation
serial to replies. The bridge publishes shared source context/seek telemetry only
from a newer native serial, including pause/play and cursor changes within one
queue generation. Native processing order wins over request start/completion
order. Each caller still receives its actual response, never a substituted cached
snapshot. This does not replace native generation/intent/media checks or the
original App request guard, nor order every caller's UI state. No new status poll,
IPC, background retry or engine command is introduced. Legacy absent serials stay
unsequenced only until the first native serial; missing or malformed serials after
that are an explicit unconfirmed-status error. The current binary always supplies
it. Exhaustion refuses ordinary operations before execution but permits shutdown.

External and embedded covers now read from a held handle while the selected root,
ancestors and file are held read-only, with checked final paths and no normal
Win32 write/delete sharing or reparse/cloud recall. The existing checked-open
code is reused without changing strict128-bit file inspection. Cover content
reads do not require FileIdInfo128 or advertise an audio/file identity revision.
This preserves compatibility intent for ordinary local filesystems; native
NTFS/FAT/removable-drive behavior still needs acceptance. Portable fixtures use
inode checks because Unix open handles do not prevent replacement.

Before publishing an image the catalog revalidates root, epoch, album and selected
cover/media paths. There is no catalog mutex across physical I/O. Progressive
catalog access, JPEG/PNG handling, external-first priority,4MiB image limit,
8MiB embedded read budget and original Lofty parser settings remain. At most66
path nodes plus a duplicate reader and one temporary validation handle are used
per call. Added metadata/handle work has not been timed on Windows; blocked OS
calls remain uncancellable. No additional media-content read, media/tag write,
MPD route, saved-data schema, new UI or Linux behavior change is introduced.

Checks include2 new portable Clock tests,8 bridge/actual-core ordering and seek
cases,9 new cover-boundary/parser/catalog cases,7 strict file-info regressions,
affected bridge regressions, MSVC --tests and TypeScript. The earlier full265
portable cases are retained as prior evidence; they were not all rerun solely
for these bounded changes. One Win32 sharing fixture was cross-compiled only.
This is not native filesystem/IPC/audio/performance/fidelity validation.


## Multiple folders and exact-handle container transport in Preview 17

The original folder settings now accept up to32 non-overlapping local fixed or
removable-drive roots. Nested/duplicate roots are explicitly refused. Album IDs,
opaque track references and one-folder catalog/covers/navigation are preserved;
different folders containing albums with the same name remain distinct. Source
labels use the actual root. A missing root on multi-root restore produces a
warning while available roots are scanned. Library checks fence the whole root
set. A scan still uses the existing progressive full-scan policy.

The owned MPD reads encoded container bytes through a private, random capability
URL on127.0.0.1. Local files are opened under checked root/ancestor/reparse/cloud
boundaries, retained by exact handle, and served without reopening paths. This
is a transport change from16, not PCM processing, conversion or DSP. GET/HEAD,
bounded single Range requests, correct206/416 responses and offset-based reads
support the MPD curl contract. Global physical readers are limited to8; headers
to8192bytes; header admission to3seconds with500ms idle and writes to2seconds.
Revocation/retirement closes sockets. OS disk I/O remains uncancellable; permits
and handles remain held until a physical reader returns. Native seeking,
gapless/transitions, filesystem sharing and performance are not yet validated.

Queue insertion/removal/reordering retains source leases until confirmation,
including partial-ACK uncertainty/rollback. The logical queue persists stable
metadata/path/root per occurrence, never the ephemeral capability URL. Data in
library-v2 and queue-v2 import legacy files only if the new file is absent;
corruption/inaccessibility does not silently fall back. Legacy files remain
rollback backups. Downgrading to16 does not show later v2 changes. Startup stays
logically paused with no MPD/media server/audio; the catalog still performs its
normal read-only scan. Explicit Play admits handles and prepares the engine.

Source facts compare strict FileIdInfo128 against the actual admitted handle,
in addition to URI/path/cursor/generation/intent. No extra media I/O or status
poll is introduced. FileId128 is optional for playback; without it strict source
facts remain unavailable. Tag/property reads use the same checked-handle boundary
while retaining prior Lofty options, limits and tags-only fallback. Parent handles
are short-lived during admission/read, not multiplied by1000 queued tracks.
MPD stdout/stderr remain valid inherited NUL handles and default Windows stdout
logging avoids raw capability URLs being persisted in engine/startup logs.

Validation:293 portable tests passed with2 previously ignored helpers; subsequent
final configuration migration checks passed5/5. Includes8 media transport/resource
cases,5 bridge root cases and29 bridge regressions, exact Preview16 one-folder
JSON comparison, MSVC --tests and TypeScript/Vite.493 original UI/Linux files are
unchanged. This is simulation/cross compilation, not native Windows/audio proof.
The maintainer reported Preview16 initially working; exceptional scenarios are
not presumed accepted. Plex, Navidrome and radio remain the next integration
lots and are not enabled as completed features in17.


## Preview 18: private diagnosis of the Preview 17 playback regression

The maintainer reported that first local playback failed in17, before adding a
second folder. The final message was the generic player.requestError.failed.
The cause is still unconfirmed. This build is a bounded diagnostic round; remote
source work remains preserved separately and is not included or enabled.

The Windows MPD curl input now explicitly disables inherited proxy settings for
its private loopback media requests. The pinned MPD0.24.18 source and libcurl
CURLOPT_PROXY contract support the override; no native fix has been confirmed.
Per-lease monotonic atomics record only coarse progress (authorized request,
GET headers, some payload or read/admission failure). MPD-reported errors use
the response cursor evidence for the relevant lease; malformed evidence keeps
the generic error. A timeout can also report coarse observed progress. These
codes do not establish a cause, complete download, decoding or successful audio.
The existing Windows error overlay can show only a fixed allowlist of codes;
raw errors, paths, capability URLs, credentials and ACK text are excluded.
No new polling, media I/O on status, logs, queue policy or shared Linux/UI edits.

Checks:9 HTTP fixture cases passed, then1 focused final-name case;1 allowlist
case, TypeScript/Vite and MSVC --tests passed. Previous multiple-root evidence
is carried forward explicitly. The next native check is only one known local
track; report whether it plays, otherwise the fixed code under the alert.
Plex, Navidrome and radio remain incomplete. Native17 is failed,18 pending.


## Preview 19: fix the native MPD NUL-output startup regression

The maintainer ran the isolated startup probe with the exact pinned MPD binary.
The16 file-output case and18 captured-file case remained running after5seconds;
17/18 NUL-output cases, including18 with an explicit log_file, exited with code3.
This isolates the NUL stdout/stderr handle as the trigger in that environment.
Wildmidi and an absent initial database were warnings in the running cases.
It does not establish which CRT/library internals caused the exit or prove audio.

The owned child now inherits a private anonymous-pipe writer for stdout/stderr.
The parent reader is non-inheritable; the existing explicit handle list still
excludes sockets, Job and media handles. Input remains NUL. The parent closes
its writer immediately after CreateProcessW, preserving EOF after child/Job exit.
A worker continuously discards bytes through a4KiB buffer: no raw output or
private capability URL is retained in a file or report. Physical readers are
limited globally to4 across old/candidate/unreturned workers; capacity remains
held until the read handle is closed and the worker returns. Drain I/O failures
produce a fixed engine_stdio_failed code during startup/status/control/checkpoint.
Cleanup waits up to100ms, joins only a finished thread, and otherwise retains
physical capacity until EOF. The OS pipe-size argument65536 is only a hint.

CreateProcess suspended/Job assignment/ResumeThread, password TCP ownership,
shutdown/termination, argument quoting, media admission, MPD configuration,
queue/journal/rollback and original Linux/UI behavior were preserved. Source
comparison proves only the stated stdio delta; WindowsSys adds only the Pipes
feature, with unchanged lockfile and Linux dependencies. No remote-source WIP
was included. Source work remains isolated pending repaired playback.

Validation:4 portable reader/resource/real-OS-pipe burst+EOF cases passed and
MSVC --tests compiled. Previous18/17 evidence is explicitly carried forward in
the receipt. The corrected pipe startup, playback and native EOF/sharing are
pending the maintainer's Windows test. First play one known file, then seek.
Only after that check should the multiple-root scenario be resumed. No new
exclusive output, PCM bridge, DSP, fidelity claim or second interface.


## Preview20: radio adapter and startup artwork

The original RadioView, radio service, directory browsing, pagination, filters,
manual stations and favorites are reused. Radio policy is a platform-adapted copy
of radio.rs with an exact source comparison; original Linux files stay unchanged.
Network requests run only on demand, with four async slots and a14-second budget.
A bounded host keeps up to16 file operations admitted through physical return,
even if their caller is cancelled. Shutdown waits up to3seconds; this is not an
OS DNS/disk cancellation guarantee. Windows TLS, actual live playback and native
file reparse/ACL behavior still require a representative Windows test.

An explicit station ID resolves in backend memory before the existing owned MPD
candidate/rollback path. Stream URLs are never filesystem paths or serialized
queue metadata. Radio has an explicit media identity, station artwork and live
ICY metadata; missing/stopped tags reset to the station. Pause stops live radio,
seek is refused, and stop is fenced to the current station and playback intent.
Local transport/configuration/stdio/auth/queue behavior is preserved comparatively.
Confirmed live radio uses the existing null v2 queue envelope, so it never resumes
on startup. An unreadable/corrupt prior checkpoint is preserved and blocks radio
admission before MPD preparation; explicit recovery remains a separate human
operation. Failed valid-cache writes retain dirty/warning and the prior disk state.
The normal Local/Favorites artwork startup race is fixed in checkpoint2977801.
All native claims remain scoped to the maintainer's Preview19 two-folder playback;
the new artwork/radio changes are not yet natively accepted. Plex/Navidrome,
output selection/exclusive, PCM/CamillaDSP and fidelity claims remain incomplete.


## Preview21: Navidrome integration
The original Navidrome settings, browser, discovery/search, favorites and player
commands now use the Windows host. Local/Navidrome library flags and folders
share an atomic configuration; account changes coordinate with the MPD worker.
Credentials use Windows CurrentUser DPAPI with no plaintext fallback. The mixed
queue stores stable account/song references in a v3 journal; v1/v2 remain read-only.
Startup restores a paused logical queue without starting MPD or a media listener.
Play resolves current account references outside the foreground worker, then
checks intent/source/lease identity before effect. Playlists retain unavailable
remote entries. Missing Local favorites use the existing missing-album UI only
after a complete clean catalog with admitted roots; references are never deleted.
Native Windows login, DPAPI, playback/TLS, mixed queue and restart still require
human validation. Navidrome smart catalog/editions, remote source facts/history,
Plex, output selection/exclusive and CamillaDSP are pending.


## Preview23: Plex artwork and insertion feedback
Plex playback was accepted on Preview22. The same authenticated metadata proof
now supplies a validated artwork reference to the original queue and player.
The reference remains presentation-only: queue-v3 is unchanged, so reopening
restores paused entries without artwork until explicit Play revalidates them.
No startup playback or network resolution was added.

Eight original artwork requests now enter a separate bounded queue, with two
image transfers, independent of four metadata/control transfers. Source changes
cancel queued image work; shutdown drains both admission classes. Image failures
remain component failures with fallback instead of raising a server-offline toast;
real catalog failures still propagate. The original insertion notice is portaled
outside cards by the Windows Vite transform, preserving original Linux source.

395 isolated module tests, eight frontend cases and MSVC/TypeScript builds pass.
The original Navidrome album view lacks queue-insertion actions; use shared
Search track menus for next/end. No separate Navidrome interface was added.
Native artwork/layout/mixed queue validation remains pending. Default WASAPI
Shared, output selection/exclusive, PCM/CamillaDSP and remote history/facts limits
remain as documented; no additional hardware capability is claimed.


### Queue artwork persistence follows Linux
The original Linux durable_queue_entry/sanitize_track_artwork already preserve
Plex image references. Windows now admits same-server, safe presentation paths
from the original queue menus without starting playback. Playback metadata still
verifies media independently. A v4 journal carries aligned optional artwork next
to the unchanged v3 logical schema, with a4MiB bound and no raw image payload,
authenticated URL, lease or connection ID. Missing v4 reads v3/v2/v1 read-only;
corruption and empty checkpoints remain authoritative. Earlier files are retained.

An existing v3 queue has no saved artwork to recover offline; after explicit
Play or a new insertion supplies artwork, v4 preserves it on subsequent reopen.
Startup remains paused, with no HTTP, engine or listener. This supersedes the
Preview23 paragraph above that described artwork as runtime-only. Windows-only
features are prohibited: new product functions must ship on Linux first, and
Windows must follow the same Linux version and shared interface.


### Recentes includes Local, Navidrome and Plex
The existing Linux listening-history identity/sanitization and HistoryView flow
now have Windows adapters for all three durable sources. Confirmed live remote
rows require a matching healthy lease; paused restores, unknown/stale leases,
radio and warning states do not record. Remote transport URIs, connection IDs,
server filesystem paths and raw covers are discarded. Stable Nav account/song
and Plex server/part identities deduplicate across sessions. Local identity,
100-entry bound, enable/clear and independent storage-worker behavior remain.

A new windows-listening-history-v2.json reads the older local-only v1 only when
absent, without rewriting it. Empty/current corrupt files remain authoritative.
Plex album navigation from Recentes rechecks the exact entry after HTTP and the
same source binding before returning. The original UI supplies replay, queue
insertion, source filters and album navigation; no Windows-only workflow exists.
Native remote history/replay/navigation and migration still need grouped testing.


### Plex declared source facts (after Preview23)
The existing player-source-info request can report declared Plex codec/rate/depth
from the exact current rating/Part, using the original Linux parser unchanged.
Only an already-resolved authenticated route is read, with one optional physical
admission, a4-second/256KiB bound, header credentials, no discovery, redirect or
retry. Current queue occurrence/revision/generation and binding are checked
before/after; stale, unknown and ambiguous facts remain unavailable. The Local
file-inspection route is preserved. Navidrome player source facts remain absent
because the Linux reference also declines them.

Plex album-track DTOs also reuse the original track_file_info parser on metadata
already received, without another request. Fixed labels/numeric facts never
establish output format, DSP, DAC lock or bit-perfect playback. These additions
are not included in the already delivered Preview23; native testing and a future
grouped package remain pending. No separate interface or Linux behavior changed.


### Preview24: original remote smart playlists and Plex file facts
The original SmartPlaylists screen/service now supports Navidrome account and
Plex server/library rules, catalog/genre/year/declared-added-date filters,
source-scoped favorites and the existing ordered/varied track expansion.
Preview results keep the original media identities and can be played or saved
through the original queue/playlist paths. Catalog reads do not prepare streams.
Native before/after generation fences reject reconnects, source changes and
obsolete expansion; Local catalog fences and error codes remain preserved.
Navidrome catalogs use the original complete pagination policy and a30-second
cache per connection; incomplete/error responses are not published as success.
Plex requires a complete inventory from the exact server/library.

Rules use windows-smart-playlists-v2.json, reading the previous Local-only v1
file only when the current file is absent. Legacy bytes remain untouched;
current empty/corrupt/linked records remain authoritative. The record shape and
version1 field remain compatible; the separate filename isolates the additional
source variants from older preview binaries. The existing atomic writer and
worker call/shutdown semantics are preserved.

This grouped preview also includes the declared Plex source/file facts described
above after Preview23. Portable fixtures and Windows compilation are evidence of
contracts, not native playback or hardware certification. Human Windows tests
remain necessary for the new smart-playlist/facts flows. Local added dates are
still unknown in the current Windows index; historical dates are not invented
from folder timestamps or the upgrade date. EQ/CamillaDSP, Direct/exclusive and
stable output selection remain pending and are not exposed as completed here.
Linux source/UI behavior and all existing MPD/queue/process/vault pins are intact.


## Preview 32 — Shared EQ fixes and Direct error observations

Windows-only corrections: Camilla Shared playback declares its required F32
format; private capture stays48kS32. MPD audio candidate3 distinguishes a fully
drained natural release from cancellation with pending data: terminal EOF is
kept, while undrained/new-data cancellation remains terminal LOST. No generation
reset/reuse, source/DSP rewrite or approved Shared runtime replacement.

The maintainer qualified all five isolated File PCM cases and the pinned Camilla
validator rejected old SharedS32/101 and accepted correctedF32/0. Empty capture
startup/readback stayed alive and cleanup closed its Job. This does not qualify
GUI playback, WASAPI physical output, active seek, track/rate transitions or Direct.
The installer requires representative native playback tests; startup stays paused.

About/Copy diagnostics now retains fixed Direct error hints from status replies
already received by the player. No extra engine query or Result/control change;
no raw errors, paths, URLs, device names or metadata are copied. Direct remains
unqualified. Original Linux/UI/locales/Cargo profile are unchanged.

Candidate3 corresponding sources are bundled as source-materials.tar.xz and
documented under windows/engines/audio-candidate-3; base2 and overlay are both
complete and explicit. Camilla2 and the approved Shared181 runtime are preserved.


### Post-Preview35: original paired LAN remote (native validation pending)

The original remote settings, browser page, pairing, expiry, revocation and
request replay policies now use the Windows playback actor. Full mode, completed
setup and the existing beta opt-in are required. The service remains disabled by
default. An explicitly enabled service restores its port at startup, matching the
Linux contract and the maintainer's specific authorization. This does not start
MPD, EQ or playback. Simple mode suspends the port while retaining saved grants;
turning beta off revokes them. Application shutdown closes the service.

Public track and queue bindings are opaque digests of the current Windows
occurrence and revision. Commands are revalidated after actor admission and source
preparation, then use the existing transport and volume actions. Direct still
refuses gradual volume; EQ transition/random limitations remain in force. This
service does not control Windows Master volume or alter firewall configuration.

Preference and hashed client records use protected user/SYSTEM file ACLs. Existing
corruption or broader permissions are refused without repair or reset. If an
explicit OFF encounters damaged preferences, the port closes but the durable save
still reports failure and preserves those bytes. Windows interface enumeration,
private ACLs, firewall access, phone pairing, lifecycle and real playback still
need grouped native testing. Preview35 and earlier installers are unchanged and
do not contain this post35 source checkpoint. The UPnP adapter described below is also a post35 change.


### Post-Preview35: original UPnP receiver (native validation pending)

The original receiver setting, bounded SSDP/SOAP/event renderer and XML policies
now use a separate volatile media lease in the Windows playback actor. Receiving
an address prepares one STOPPED track and never implicitly plays or seeks. Play,
pause, stop, seek and permitted software gain use existing actions, fenced to the
receiver session after admission. Replacing the desktop queue checks candidate
preparation and owned cleanup before replacing its durable checkpoint. Failures
after a cut fall back to the last logical queue paused, without automatic replay.
The received media URL is backend-only and never saved as a local track or
playlist; receiver playback does not become a Local history/scrobbling entry.

Full mode and completed setup are required; beta is not required. The receiver
is disabled by default. Only an explicitly enabled preference restores network
ports at startup, without an audio engine or autoplay. Simple mode and OFF close
ports but preserve already admitted media for desktop playback. Unrelated config
saves preserve active controller subscriptions. Service startup waits for backend
readiness asynchronously, including the paired remote, and shutdown cancels that
wait before closing and joining workers. No firewall setting is changed.

Portable protocol, own loopback HTTP/events, volatile media and stopped-cut
failure cases passed, as did Windows cross compilation and frontend checks.
Windows multicast/interface enumeration, firewall, ACL, real controller/audio,
Direct/EQ receiver transitions and lifecycle still require grouped native tests.
The next private installer will be generated only at the end of the round.

### Post-Preview35: explicit EQ track navigation

Explicit Next now advances even with repeat-one selected, matching MPD's native
command; repeat-one still controls natural completion. Previous at the first
track restarts that track when repeat is off, and wraps only when repeat is on.
This uses the existing fresh-generation transaction, queue, presets and options.
Random Next/Previous remains guarded pending same-MPD order preservation. The
portable before/after cases and rollback checks passed; native EQ navigation is
included in the next grouped manual test.

### Post-Preview35: fixed Direct stderr observations

MPD can collapse a detailed output error to `Failed to open audio output` in its
status response. The existing owned pipe drain now recognizes only authored MPD
Direct-output line prefixes and a finite set of markers/HRESULTs. Copy Diagnostic
can retain `WIN_DIRECT_STDIO_*` hints without retaining or printing the raw log,
URL, device name or metadata. Foreign domains/names, oversized/partial lines and
unknown codes do not gain a specific attribution. These observations can include
a candidate attempt and do not prove a physical cause or the current output.

The pipe reader publishes only atomic bits; its owner records the fixed hints
during existing checks/cleanup. It does not wait for diagnostic locks, add reads
or threads, change IPC errors, retry playback, alter gain or recover a route.
The original four-reader capacity and physical-EOF ownership remain in place.
Native Direct/device validation is still pending.

### Post-Preview35: selected-track navigation after a paused route change

Next, Previous and queue selection discard the prior staged resume position
before starting the selected track. Explicit Resume retains that old position.
A logically paused fresh engine is kept distinct from a physically playing MPD,
so repeat navigation no longer sends a native step to a stopped candidate.
Restored repeat wraps consistently; Previous at the first non-repeat track
restarts it. Existing playback confirmation and persistence transactions remain.

Random Next/Previous uses the owned live MPD only. If its order is unavailable
after restoration/stop, the action is refused before gain or playback effects;
play/select a track first. No ordinal target is invented for a random queue.
EQ random stepping remains separately guarded. The terminal non-repeat Next
stop/cursor case is still pending and is not claimed fixed by this change.

### Post-Preview35: bounded confirmation of missing favorites

An album removed from a clean local catalog remains a saved favorite and uses
the original missing-album presentation. Confirming that absence now shares one
six-second filesystem budget across all configured folders. A timed-out worker
retains its physical admission slot until it returns and cannot publish a late
success. Folder identity/admission checks remain unchanged, including SMB.

Before publishing absence, the catalog rechecks its epoch, revision, roots,
signature, warnings and local-source state. An unavailable folder, partial scan
or superseded request returns an error, preserving the favorite. This change does
not establish that the reported native network-folder failure is resolved.

### Post-Preview35: playlist file availability uses playback admission

Local playlist files now pass the existing held ancestor, identity, reparse and
SMB checks before being reported available. A canonical path prefix alone is
insufficient. An explicitly absent known file also needs a held, validated parent
for its negative lookup; a network error or reparse ancestor remains unavailable.
Windows network/path errors other than file/path-not-found do not prove removal.

One live-root proof is shared within the existing six-second availability
request. An unknown catalog URI requires a clean complete catalog; an explicitly
absent known file can still be identified when unrelated tags have warnings.
The proof rechecks catalog version, roots, signature, warnings and enabled source,
and is never serialized or cached for a later request. No playlist is rewritten
or pruned. Windows filesystem/SMB validation remains pending.

### Post-Preview35: preserve staged cursors through command admission

A refused Direct gain/local start admission no longer discards the paused
selection or resume position. Once admitted, explicit track selection still
starts at zero; explicit Resume keeps its saved position. Stop commits a staged
zero cursor only after both Stop and ClearError acknowledgements. A failed
acknowledgement returns the original error without Play, Seek or a retry.

This concerns failures before playback admission and staged Stop publication;
it does not establish rollback after an accepted or ambiguous Play command.

## Preview36 grouped checkpoint

Groups the original paired LAN remote and UPnP receiver adapters, explicit EQ
navigation corrections, staged selection/admission fixes, fixed Direct stderr
observations, bounded missing-favorite confirmation and held-file playlist
availability. The Linux0.6.2 UI/core and approved runtime pins are preserved.
Network services stay disabled by default; explicitly enabled services may
restore their ports on startup, with no automatic playback.

Native validation of these deltas is pending. The audible EQ acceptance on
Preview32 remains valid for that tested version. Preview35/current analyzer,
network folders, shared Plex accounts, Direct, remote/UPnP and their lifecycle
cases still need the grouped Windows test. EQ random stepping and active Shared
to EQ retain their guards. Default-follow, Master control, production metadata
writes and full library recovery are not claimed implemented by this package.
File-only qualification tools are separate from the installed application.

## Preview37: remote desktop work leaves the UI thread

Preserves the grouped Preview36 checkpoint and moves all five remote desktop
commands to bounded blocking work: four read reservations and one independent
write reservation. A caller disappearing does not release a live worker's slot.
Main-window, mode and lifecycle checks remain; original mutation/QR policy and
wire schemas are unchanged. Grant revocation and native adapter enumeration no
longer run directly in the desktop command callback.

Mutation locks and physical write admission are released before reading fresh
network status. A committed action followed by an unavailable status read still
returns an error; it is not retried or described as confirmed. This is bounded
admission, not a promise that an uninterruptible OS/storage call can be cancelled.
Native GUI, network-driver, grant persistence and shutdown tests remain pending.
Audio code and runtime pins are byte-exact to Preview36. Separate Session3 tools
retain that exact Preview36 source reference and do not qualify the new UI worker
or a physical output.
