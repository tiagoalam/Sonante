# Gapless validation scope

`scripts/verify-gapless.py` provides a repeatable, sample-exact engine test.
It generates a deterministic stereo PCM signal, encodes a whole-track FLAC and
two split FLAC tracks, and captures an explicitly selected MPD engine through its
FIFO output. The split is deliberately not aligned to common decoder block sizes.
The engine executable must match the SHA-256 supplied to the script before it runs.

For 16-bit/44.1 kHz and 24-bit/96 kHz pairs, success requires both:

1. The single-track decoded PCM exactly matches the generated reference.
2. The two-track decoded PCM exactly matches that single-track capture, including
   byte count and every sample across the boundary.

Crossfade, random, repeat, single and consume are disabled. The process uses only
its generated files, private temporary configuration, Unix socket and FIFO. Its
only MPD indexing request concerns those small synthetic fixtures. Cleanup owns
its child process directly; it never signals a PID obtained from a profile.

The report identifies the engine hash, formats, frame counts, boundary and output
hashes. A successful result demonstrates sample continuity of that engine's FLAC
decoder and FIFO path for the measured pairs. It does **not** establish physical
DAC clock/output continuity, gapless across changed formats, lossy-codec padding,
or behavior of Shared, EQ, Plex or UPnP. Those require separate evidence.

No unqualified player-wide gapless badge is inferred from this test.
