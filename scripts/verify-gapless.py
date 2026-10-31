#!/usr/bin/env python3
"""Measure MPD FIFO sample continuity using owned synthetic fixtures only.

This tests the selected engine/decoder/FIFO path, not a physical DAC, Shared,
EQ, Plex, compressed-codec padding or transitions between sample rates.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import tempfile
import threading
import time
import wave


def command(path, value):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(3)
        connection.connect(str(path))
        with connection.makefile("rb") as reply:
            if not reply.readline(256).startswith(b"OK MPD "):
                raise RuntimeError("Invalid owned engine greeting")
            connection.sendall((value + "\n").encode())
            lines = []
            for _ in range(1024):
                line = reply.readline(8192)
                if line == b"OK\n":
                    return lines
                if not line or line.startswith(b"ACK"):
                    raise RuntimeError("Owned engine refused command or closed reply: " + line.decode(errors="replace")[:512])
                lines.append(line.decode().strip())
            raise RuntimeError("Reply bound exceeded")


def measure(engine, root, rate, bits):
    folder = root / f"pcm{bits}-{rate}"
    folder.mkdir()
    frames = rate + 1009
    split = rate // 3 + 17  # deliberately not aligned to common decoder blocks
    raw = bytearray()
    for n in range(frames):
        for channel in range(2):
            sample = ((n * 7919 + channel * 104729) % (1 << (bits - 2))) + 1
            if n % 3 == 0:
                sample = -sample
            raw += sample.to_bytes(bits // 8, "little", signed=True)
    for name, payload in [("whole", raw), ("first", raw[:split * 2 * (bits // 8)]), ("second", raw[split * 2 * (bits // 8):])]:
        wav = folder / f"{name}.wav"
        with wave.open(str(wav), "wb") as output:
            output.setnchannels(2)
            output.setsampwidth(bits // 8)
            output.setframerate(rate)
            output.writeframes(payload)
        subprocess.run(["flac", "--silent", "--force", str(wav)], check=True, timeout=10, capture_output=True)
    fifo, ipc = folder / "capture.fifo", folder / "mpd.sock"
    os.mkfifo(fifo, 0o600)
    outbits = 16 if bits == 16 else 32
    config = folder / "mpd.conf"
    config.write_text(f'''music_directory "{folder}"
db_file "{folder}/db"
bind_to_address "{ipc}"
log_file "/dev/null"
auto_update "no"
zeroconf_enabled "no"
audio_output {{
 type "fifo"
 name "Owned sample measurement"
 path "{fifo}"
 format "{rate}:{outbits}:2"
 mixer_type "none"
}}
''')
    fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
    stop = threading.Event()
    captured = bytearray()
    guard = threading.Lock()

    def reader():
        while not stop.is_set():
            ready, _, _ = select.select([fd], [], [], .02)
            if ready:
                block = os.read(fd, 65536)
                if not block:
                    stop.wait(.002)
                else:
                    with guard:
                        captured.extend(block)
                        if len(captured) > 8 * 1024 * 1024:
                            stop.set()

    thread = threading.Thread(target=reader)
    thread.start()
    process = None
    try:
        process = subprocess.Popen([str(engine), "--no-daemon", str(config)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 5
        while not ipc.exists():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("Owned FIFO-only engine did not start: " + (process.stderr.read(8192).decode(errors="replace") if process.poll() is not None else "startup timeout"))
            time.sleep(.02)
        command(ipc, "update")
        deadline = time.monotonic() + 3
        while any(line.startswith("updating_db:") for line in command(ipc, "status")):
            if time.monotonic() > deadline: raise RuntimeError("Synthetic fixture indexing timeout")
            time.sleep(.02)
        command(ipc, "command_list_begin\ncrossfade 0\nsingle 0\nrepeat 0\nrandom 0\nconsume 0\ncommand_list_end")

        def play(names):
            with guard:
                captured.clear()
            batch = "command_list_begin\nclear\n" + "".join(f'add "{name}.flac"\n' for name in names) + "play 0\ncommand_list_end"
            command(ipc, batch)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                state = command(ipc, "status")
                with guard:
                    size = len(captured)
                if "state: stop" in state and size:
                    time.sleep(.1)
                    with guard:
                        return bytes(captured)
                time.sleep(.01)
            raise RuntimeError("Sample capture timed out")

        whole = play(["whole"])
        split_output = play(["first", "second"])
        expected = bytes(raw) if bits == 16 else b"".join(struct.pack("<i", int.from_bytes(raw[n:n + 3], "little", signed=True) << 8) for n in range(0, len(raw), 3))
        return {"rate": rate, "input_bits": bits, "output_bits": outbits, "channels": 2,
                "tracks": 2, "frames": frames, "boundary_frame": split,
                "reference_bytes": len(whole), "split_bytes": len(split_output),
                "reference_matches_generated_signal": whole == expected,
                "split_matches_reference": split_output == whole,
                "sha256": hashlib.sha256(split_output).hexdigest(),
                "pass": whole == expected and split_output == whole}
    finally:
        if process is not None:
            if process.poll() is None:
                process.terminate()  # only our Popen child, never a PID file
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        stop.set()
        thread.join(timeout=1)
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    engine = args.engine.resolve(strict=True)
    actual = hashlib.sha256(engine.read_bytes()).hexdigest()
    if actual != args.sha256.lower():
        raise SystemExit("Engine hash mismatch; no execution")
    with tempfile.TemporaryDirectory(prefix="sonante-gapless-") as folder:
        results = [measure(engine, Path(folder), 44100, 16), measure(engine, Path(folder), 96000, 24)]
    report = {"engine_sha256": actual, "path": "FLAC decode -> MPD FIFO PCM; same format per pair",
              "results": results, "pass": all(result["pass"] for result in results),
              "limitations": ["No physical output or clock continuity measurement", "No Shared/EQ/Plex/UPnP or lossy formats qualified", "No format-changing transition qualified"]}
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
