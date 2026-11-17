CamillaDSP 4.1.3 — Sonante Windows private PCM/WASAPI/analyzer candidate3

Complete corresponding source, original licenses and 358 vendored packages are
included. The source/ directory is already patched: do NOT apply patches again.
The base2 adapter and the analyzer3 delta are included for audit with before/after
hashes. The exact delta was reapplied without fuzz to an affected-file fixture.
All 21561 declared vendor checksums and all source members were verified.

DSP/filter/audio sample conversion/rendering and sole private PCM ring reader
remain unchanged. The optional Windows observer copies already-owned PCM after
the existing writable event, uses bounded nonblocking capture offers and one
lazy math worker. Only RMS/peak/48-band metrics traverse the existing private
control websocket. The Linux0.6.2 analyzer constants/core/math are byte-exact;
compile-time source drift guards require review when that base changes.
The calculation is pre-DSP on the actual MPD PCM (including MPD software gain),
not an output-device meter. It is unavailable without a current real source.

Only a Windows-target direct edge to already-locked RustFFT6.4.1 was added.
Dependency versions, vendor bytes, resolved features, non-Windows graph and
original release profile are unchanged. See dependency-review.json.

Verify sources without running any engine:
  python3 rebuild.py --verify-only
Cross-build from verified source (does not execute the EXE):
  python3 rebuild.py --build
Prerequisites: Rust/Cargo1.99, MSVC target, cargo-xwin, LLVM/lld and already
verified Windows SDK/CRT cache. Cargo uses included vendor, --locked --offline,
static CRT, original release profile, nice10 and one job. The receipt records a
fresh complete-source cross-build; it does not assert byte reproducibility.
Windows IPC/WASAPI, interactive behavior and hardware require separate testing.

Original CamillaDSP source offers GPL-3.0-only OR MPL-2.0; original texts retain
original filenames in source/. This combined candidate is supplied under the
GPL-3.0-only option, including the new observer and original Sonante analyzer.
See LICENSE-sonante-analyzer, AUTHORS-sonante-analyzer and source/vendor for
applicable notices and third-party licenses. No user data or credentials included.
