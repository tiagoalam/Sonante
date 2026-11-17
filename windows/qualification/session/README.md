# Windows native Session qualification (File output only)

This is an explicit test executable, not a product mode or a second UI. The
ignored test `windows_mpd::session::native_qualification::isolated_session_file_qualification`
uses the actual product Session, Process/Job/stdio, TCP ownership proof, private
ring guardian, local media leases, queue edits and journal. The File sink override
is thread-scoped and compiled only in the Windows **test** executable. Release
applications have no environment/config switch that can select this sink.

Run only the provided explicit qualification script from the **current** Session
qualification bundle. Its `runtimes` directory must be immediately beside the
test executable and must contain the exact current MPD and Camilla pins. Old
Session2/PCM-Correcao1 bundles are preserved controls and do not qualify the
current Camilla3/analyzer. The tool accepts only these relative pinned engine
assets, copies them to its own private temporary fixture, creates synthetic
WAV files, and opens no audio endpoint or application/user profile. It checks:

- stopped startup and staged paused queue with zero PCM and no Play/Seek;
- insertion/reordering/removal with the actual MPD queue and retained source flags;
- logical journal round-trip and unchanged journal after source-server retirement;
- independent guardian shutdown after a retained source listener closes;
- normal closure of both exact owned Jobs;
- MPD status-error shutdown using an owned loopback HTTP404 fixture.

The separate ignored test
`isolated_current_session_analyzer_file_qualification` also drives an actual
Session with an owned, paced synthetic HTTP WAV through the existing UPnP media
lease, private PCM, current Camilla3 Flat render and File output. It checks two
fresh nonzero analyzer frames through the real epoch/token/sequence decoder,
pause and terminal drain, every accepted sample against the original Flat gain,
stale-reader refusal and owned Job/source cleanup. The synthetic stream is needed
because an unpaced File sink can finish before a player start observation; it
does not replace or mock the product decoder, DSP, ring or analyzer.
It stages an old 42.5s cursor with zero PCM, then invokes the actual selected-track
start method. The forward-only stream and prefix comparison expose an inherited
seek/skip; this remains an ignored native test, not an executed result here.

This does **not** test audible playback, WASAPI/Direct/DAC, live presets, active
seek continuity, SSDP/SOAP discovery or the frontend. Only the additional current
File case checks sample accuracy; the original stopped fixture emits zero PCM.
The existing PCM
Qualification2 provides complementary synthetic pass-through/pause/seek/EOF tests.
Compilation is not a successful native test. A post26 source checkpoint adds the
container-server exit flag; that fix is not in the already sealed Preview26 app.

## Rebuilding the test executable

Use the exact attached full source archive, Cargo.lock and package-lock.json,
Windows x64 Rust/MSVC build tools, and Node/npm. Do not run tests automatically.
Build the Windows frontend with `npm ci` and `npm run build:windows-preview`.
Then, from the project root, run:

```text
cargo test --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc --lib --release --no-run --locked -j1 --message-format=json-render-diagnostics
```

The JSON compiler artifact for the `sonante_lib` lib-test target supplies the
executable path. This uses the original release profile; do not change it.
Cross-builds use the same command through cargo-xwin with verified SDK/toolchains.
The distributed receipt separately records source/engine/test-binary hashes and
whether any native execution occurred. Every source and prior artifact is retained.
