# WavPack in the Local SQLite library

The SQLite scanner recognizes `.wv` files and reads bounded APEv2 metadata
(or ID3v1 when no APEv2 tag is present) plus stream properties through the
existing Lofty dependency. Artwork blocks are not loaded. `.wvc` correction
files are not indexed as independent tracks, and this catalog change does not
edit audio tags or audio files.

The parser version is tracked separately so previously unsupported WavPack
records can be retried during normal source verification. CUE parsing uses the
existing source-scoped scanner and file-identity checks. A single-image CUE
replaces its physical long track; a single CUE that accounts for every file
in a matching multi-file album can also replace those physical rows. Partial
or competing CUE sheets leave the physical album as the canonical browser
entry. CUE paths, including names with spaces and Unicode, remain relative to
the authorized source.

Virtual CUE tracks use the existing transient `sonante-cue-range` transport:
the verified physical path and 75 Hz CUE frame bounds are converted by the
MPD supervisor to the private `rangeid` command. The catalog does not split
or rewrite files. Synthetic tests cover single-image and complete multi-file
CUE browsing, deduplication, persistence, incremental parser recovery, and
range resolution. They do not start MPD or touch a real library.

The virtual range model uses each AUDIO track's `INDEX 01` as its start and
the next AUDIO `INDEX 01` for the same file as its end. `INDEX 00` does not
become a separate track or move that boundary. For adjacent tracks in one
file, frames between `INDEX 00` and `INDEX 01` therefore fall within the
preceding range. If a new `FILE` begins, the previous range ends at that
file's end and the new file's pre-`INDEX 01` frames are not exposed as a
separate virtual range. `PREGAP` and `POSTGAP` directives are not synthesized
as silence. These rules are characterized by parser tests and should be
considered a CUE limitation.

## Playback qualification boundary

The 0.7.2 package receipts confirm that the two Linux MPD runtime pins were carried forward unchanged from the 0.7.1 binaries. Those binaries were checked against the pinned runtime manifests: `mpd-direct` SHA-256
`7bf2a6c96fad672ee6a056335c1b8054a71107975c637b76f549751b43d0d4b1` and
`mpd-eq` SHA-256
`6b9cb9820c1704175af312377fae6f20ad80486f69ee17cd190bc6390bc1d5ff`.
Both `--version` outputs list the native `[wavpack] wv` decoder and the
FFmpeg `wv` decoder; both dynamically depend on `libwavpack.so.1`.

Each exact binary was then exercised in an isolated MPD instance with a
synthetic WavPack file and `audio_output type "null"`. Both decoded and played
a single `.wv`, paused, sought, played a ranged image track, advanced to the
next range, and advanced between two WavPack files (including a path with
spaces and Unicode). Private Unix sockets, temporary databases and synthetic
media were used; no ALSA device, installed service, or user library was
accessed. This establishes software decoding and the exercised MPD controls
for those unchanged runtime binaries, not sound quality or hardware playback.

The 0.7.2 package receipts confirm these runtime hashes are unchanged. The CI also checked package installation and loaders; those checks do not measure sound quality or hardware playback. Hybrid correction
through `.wvc` and DSD-in-WavPack are not qualified; no lossless or DSD claim
is made for those cases.
