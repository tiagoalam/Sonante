Sonante Windows MPD audio candidate3: derived from pinned completeFull2.
This archive includes exact full source2 with all upstream source/dependency
archives, source patch, manifests and licenses; plus ONE Windows-only PCM
lifecycle overlay and offline reconstruction/full-build recipe.
 python3 rebuild-pcm-eof.py --prepare-only
or in a fresh extracted directory with matching pinned LLVM-MinGW/build tools:
 python3 rebuild-pcm-eof.py --build --toolchain /path/to/llvm-mingw-20260616-ucrt-ubuntu-22.04-x86_64
Recipe never runs Windows executables. It reconstructs base2, verifies its
previous plugin digest, applies the EOS overlay, verifies its exact four files
and follows original codec/dependency/meson/release options with nice10/j1.
Original fullsource2 licenses and exact archives are inside mpd-base2.tar.xz.
SOURCES.json separates historical base2 metadata from candidate3 provenance.

Delivered binary was built by compiling only changed plugin with original
release flags, creating a private archive and linking against retained complete
Full2 codec objects/libs. A control relink matched all original bytes except
exactly two PE timestamp fields; no executable binary patching was done.
A separate source reconstruction compared1964files, only PCM plugin CXXchanged.
Byte-identical full rebuild across toolchains/timestamps is not claimed.

Maintainer Windows File-only qualification passed5cases: exact pass-through,
Flat -1dB tolerance1LSB, pause/resume10000003frames exact, initial seek48000
frames exact, consumer-death owned cleanup. Not GUI/Wasapi/physical/bit-perfect
qualification, not active-seek/order/history/format/hotplug qualification.
Agent executed only own-memory fixtures and cross-builds, never Windows EXEs,
Sonante/MPD/Camilla playback, Linux audio or user data. DSP source unchanged.
