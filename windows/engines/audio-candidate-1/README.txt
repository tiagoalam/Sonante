Sonante Windows audio candidate — corresponding source materials

Candidate MPD 0.24.18, Windows-only stable endpoint / strict WASAPI and private
PCM producer patch. This archive is independent of the approved Shared runtime.
No Windows executable or audio hardware was run by the cross-build workflow.

All runtime source archives, Meson wrap overlays, Windows patch and build recipes
are included. SOURCES.json records digests and authoritative source provenance.
The MPD base is the official 0.24.18 archive. Zlib receives the original MPD
no_static_suffix CMake naming patch. Codec configure flags are reused from MPD.

Rebuild on a Linux x86_64 build host in a fresh extracted directory. Use the
pinned LLVM-MinGW UCRT toolchain and Meson/Ninja versions in SOURCES.json, plus
Python 3, CMake 4.4.4, make, pkg-config, git and patch. Toolchain/tools are build
inputs, not runtime dependencies. No global installation is required.

  python3 rebuild.py --prepare-only

verifies/extracts all supplied archives and applies patches, without building or
running any player. For a build, start from another fresh extraction and use:

  python3 rebuild.py --build --toolchain /absolute/path/to/llvm-mingw-20260616-ucrt-ubuntu-22.04-x86_64

The build is static, release profile unchanged, nice10/j1; Windows executables
are never executed. The result is build/mpd.exe. Exact binary reproducibility is
not claimed merely from source preparation/compilation. Inspect BUILD-RECEIPT.json
for the observed original binary digest and imported Windows/UCRT libraries.

Licenses remain in each original archive/source tree. MPD GPL-2.0-or-later;
FFmpeg configured with the original MPD GPL-enabled audio decoder flags;
libsndfile/mpg123 LGPL; libsamplerate BSD-2-Clause; curl MIT-like; zlib zlib;
FLAC/ogg/fmt and Meson overlays retain their included upstream licenses.
The complete original license texts accompany this source and the installer.
Native Direct support, endpoint busy/format behavior and physical output require
separate Windows/hardware validation. No bit-perfect attestation is made.
