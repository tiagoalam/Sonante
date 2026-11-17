#!/usr/bin/env bash
set -euo pipefail
exec python3 - "$@" <<'PYTHON'
from __future__ import annotations
import argparse
import hashlib
import os
import re
import shutil
import signal
import socket
import stat
import subprocess
import tempfile
import time
import wave
from decimal import Decimal
from pathlib import Path

PIN = "f7157589c34c8c98e8989112e1e53284dee6abdba83c5bf4b894592d134674b0"
DEFAULT_MPD = Path.home() / ".local/share/sonante-0.6.4-sqlite/runtime/eq-dev/mpd-eq"
OUTPUT = "sonante-cue-proof-null"
RATE, FPS = 44100, 75
TOL = 0.020

class Failure(RuntimeError):
    pass

def log(message):
    print(message, flush=True)

def cue_time(frames):
    whole, frame = divmod(frames, FPS)
    minute, second = divmod(whole, 60)
    return f"{minute:02d}:{second:02d}:{frame:02d}"

def cue_seconds(frames):
    return Decimal(frames) / Decimal(FPS)

def sec_text(value):
    return f"{value:.12f}".rstrip("0").rstrip(".")

def wav(path, duration):
    with wave.open(str(path), "wb") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(RATE)
        block = b"\0\0\0\0" * 8192
        left = RATE * duration
        while left:
            n = min(left, 8192)
            f.writeframesraw(block[:n * 4])
            left -= n

def cue_fixtures(music, direct):
    one = music / "single-image.wav"
    wav(one, 20)
    one_cue = music / "single-image.cue"
    starts, ends = [0, 501, 1002], [501, 1002, 1500]
    lines = ['PERFORMER "Synthetic Fixture"', 'TITLE "Single File Proof"', 'FILE "single-image.wav" WAVE']
    expected = []
    for i, (start, end) in enumerate(zip(starts, ends), 1):
        title = f"Synthetic Segment {i:02d}"
        lines += [f"  TRACK {i:02d} AUDIO", f'    TITLE "{title}"',
                  f'    PERFORMER "Synthetic Artist {i:02d}"', f"    INDEX 01 {cue_time(start)}"]
        expected.append({"title": title, "audio": one, "start": start, "end": end})
    one_cue.write_text("\n".join(lines) + "\n", encoding="utf-8")

    a, b = music / "multi-a.wav", music / "multi-b.wav"
    wav(a, 10)
    wav(b, 10)
    multi_cue = music / "multi-image.cue"
    multi_cue.write_text("\n".join([
        'PERFORMER "Synthetic Fixture"', 'TITLE "Multi File Proof"',
        'FILE "multi-a.wav" WAVE', "  TRACK 01 AUDIO",
        '    TITLE "Multi Segment 01"', "    INDEX 01 00:00:00",
        "  TRACK 02 AUDIO", '    TITLE "Multi Segment 02"',
        f"    INDEX 01 {cue_time(375)}", 'FILE "multi-b.wav" WAVE',
        "  TRACK 03 AUDIO", '    TITLE "Multi Segment 03"', "    INDEX 01 00:00:00",
    ]) + "\n", encoding="utf-8")
    multi = [
        {"title": "Multi Segment 01", "audio": a, "start": 0, "end": 375},
        {"title": "Multi Segment 02", "audio": a, "start": 375, "end": 750},
        {"title": "Multi Segment 03", "audio": b, "start": 0, "end": 750, "whole_file": True},
    ]

    direct_one = direct / "absolute-single.wav"
    direct_a, direct_b = direct / "absolute-multi-a.wav", direct / "absolute-multi-b.wav"
    wav(direct_one, 20)
    wav(direct_a, 10)
    wav(direct_b, 10)
    single_direct = [dict(x, audio=direct_one) for x in expected]
    multi_direct = [dict(x, audio=direct_a if x["audio"].name == "multi-a.wav" else direct_b) for x in multi]
    return (one_cue, expected, single_direct), (multi_cue, multi, multi_direct)

def self_check():
    with tempfile.TemporaryDirectory(prefix="sonante-cue-selfcheck-") as name:
        root = Path(name)
        music, direct = root / "music", root / "direct-only"
        music.mkdir(mode=0o700)
        direct.mkdir(mode=0o700)
        for dirname in ("playlists", "database", "state", "logs", "home", "config", "data",
                        "cache", "xdg-state", "runtime", "tmp"):
            (root / dirname).mkdir(mode=0o700)
        (one_cue, one, one_direct), (multi_cue, multi, multi_direct) = cue_fixtures(music, direct)
        assert [x["end"] - x["start"] for x in one] == [501, 501, 498]
        assert [x["end"] - x["start"] for x in multi] == [375, 375, 750]
        assert range_pair("0.000-6.680") == (0.0, 6.68)
        assert range_pair("6.680-13.360") == (6.68, 13.36)
        assert range_pair("13.360-") == (13.36, None)
        validate_range({"range": "0.000-6.680", "duration": "6.680"}, one[0], "self-check fractional")
        validate_range({"range": "13.360-", "duration": "6.640"}, one[2], "self-check open end")
        validate_range({"duration": "10.000"}, multi[2], "self-check full-file without Range")
        try:
            validate_range({"duration": "5.000"}, multi[0], "self-check missing partial Range")
        except Failure:
            pass
        else:
            raise AssertionError("missing Range must remain an error for a partial segment")
        for invalid in (None, "", "-6.680", "0.000--6.680", "0.000-6.680-",
                        "6.680-6.680", "13.360-12.000", "NaN-Inf"):
            try:
                range_pair(invalid)
            except Failure:
                pass
            else:
                raise AssertionError(f"Range inválido aceito: {invalid!r}")
        assert one_cue.is_file() and multi_cue.is_file()
        assert all(not x["audio"].resolve().is_relative_to(music.resolve()) for x in one_direct + multi_direct)
        for path, duration in [(one[0]["audio"], 20), (multi[0]["audio"], 10),
                               (multi[2]["audio"], 10), (one_direct[0]["audio"], 20)]:
            with wave.open(str(path), "rb") as f:
                assert (f.getnchannels(), f.getsampwidth(), f.getframerate(), f.getnframes()) == (2, 2, RATE, RATE * duration)
        config = config_text(root)
        root_info = inode_details(root)
        assert root_info["exists"] and root_info["type"] == "directory"
        assert root_info["uid"] == os.getuid() and int(root_info["mode"], 8) & 0o077 == 0
        assert inode_details(root / "not-created")["exists"] is False
        assert private_layout(root) == []
        os.chmod(root / "database", 0o755)
        assert any("database" in problem for problem in private_layout(root))
        os.chmod(root / "database", 0o700)
        assert f'bind_to_address "{root / "mpd.sock"}"' in config
        assert not any(line.strip().startswith("port ") for line in config.splitlines())
        assert 'unix_socket' + '_permissions' not in config
        assert 'type "null"' in config
        assert config.count("audio_output {") == 1 and config.count("bind_to_address ") == 1
        assert "alsa" not in config.lower() and "pipewire" not in config.lower()
    log("SELF-CHECK OK: WAV/CUE, limites exatos, arquivo absoluto fora do banco MPD e configuração socket/null. MPD não iniciado.")

def quote(s):
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"') + '"'

class Client:
    def __init__(self, path, transcript):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5)
        self.sock.connect(str(path))
        self.reader = self.sock.makefile("r", encoding="utf-8", errors="replace")
        self.log = transcript
        greeting = self.reader.readline().rstrip()
        self.log.write(f"<<< {greeting}\n")
        self.log.flush()
        if not greeting.startswith("OK MPD "):
            raise Failure(f"Saudação MPD inesperada: {greeting}")

    def cmd(self, name, *args):
        words = [quote(x[1]) if isinstance(x, tuple) and x[0] == "q" else str(x) for x in args]
        command = name + ((" " + " ".join(words)) if words else "")
        self.log.write(f">>> {command}\n")
        self.sock.sendall((command + "\n").encode())
        response = []
        while True:
            line = self.reader.readline()
            if not line:
                raise Failure(f"Conexão interrompida durante {name}.")
            line = line.rstrip("\r\n")
            response.append(line)
            if line == "OK" or line.startswith("ACK "):
                break
        self.log.write("<<< " + "\n<<< ".join(response) + "\n")
        self.log.flush()
        if response[-1].startswith("ACK "):
            raise Failure(f"ACK em {name}: {response[-1]}")
        return response[:-1]

    def close(self):
        self.reader.close()
        self.sock.close()

def blocks(lines, first="file"):
    out, current = [], None
    for line in lines:
        if ": " not in line:
            continue
        key, value = line.split(": ", 1)
        if key.lower() == first:
            if current:
                out.append(current)
            current = {first: value}
        elif current is not None:
            current[key.lower()] = value
    if current:
        out.append(current)
    return out

def one_block(lines):
    found = blocks(lines)
    if len(found) != 1:
        raise Failure(f"Resposta não corresponde a um item: {found!r}")
    return found[0]

def number(value):
    match = re.search(r"-?\d+(?:\.\d+)?", value or "")
    return float(match.group(0)) if match else None

def range_pair(value):
    pattern = r"((?:\d+(?:\.\d*)?|\.\d+))-((?:\d+(?:\.\d*)?|\.\d+))?"
    match = re.fullmatch(pattern, value.strip()) if isinstance(value, str) else None
    if not match:
        raise Failure(f"Range MPD inválido: {value!r}")
    start = Decimal(match.group(1))
    end = Decimal(match.group(2)) if match.group(2) is not None else None
    if not start.is_finite() or start < 0 or (end is not None and (not end.is_finite() or end <= start)):
        raise Failure(f"Range MPD inválido ou não crescente: {value!r}")
    return float(start), float(end) if end is not None else None

def close_enough(actual, expected, what):
    if actual is None or abs(actual - expected) > TOL:
        raise Failure(f"{what}: observado {actual!r}; esperado {expected:.6f}s (tolerância {TOL:.3f}s).")

def validate_range(item, want, label):
    duration = number(item.get("duration")) or number(item.get("time"))
    if item.get("range") is None and want.get("whole_file"):
        if want["start"] != 0:
            raise Failure(f"{label}: Range ausente em segmento que não cobre o arquivo inteiro.")
        with wave.open(str(want["audio"]), "rb") as source:
            full_duration = source.getnframes() / source.getframerate()
        expected_duration = float(cue_seconds(want["end"] - want["start"]))
        close_enough(full_duration, expected_duration, f"{label} extensão da fixture full-file")
        close_enough(duration, full_duration, f"{label} duração do arquivo inteiro")
        return duration, None
    expected_start = float(cue_seconds(want["start"]))
    expected_end = float(cue_seconds(want["end"]))
    pair = range_pair(item.get("range"))
    close_enough(pair[0], expected_start, f"{label} Range inicial")
    if pair[1] is None:
        with wave.open(str(want["audio"]), "rb") as source:
            source_duration = source.getnframes() / source.getframerate()
        close_enough(expected_end, source_duration, f"{label} limite CUE aberto vs duração real do WAV")
        expected_duration = source_duration - expected_start
        close_enough(duration, expected_duration, f"{label} duração efetiva do Range aberto")
    else:
        close_enough(pair[1], expected_end, f"{label} Range final")
        close_enough(duration, expected_end - expected_start, f"{label} duração")
    return duration, pair

def check_items(client, ids, expected, label, direct=False, cue_metadata=False):
    for i, (item_id, want) in enumerate(zip(ids, expected), 1):
        item = one_block(client.cmd("playlistid", item_id))
        duration, pair = validate_range(item, want, f"{label} faixa {i}")
        if direct and not item.get("file", "").endswith(want["audio"].name):
            raise Failure(f"{label} faixa {i}: addid não apontou ao arquivo esperado: {item!r}")
        if cue_metadata and item.get("title", "").casefold() != want["title"].casefold():
            raise Failure(f"{label} faixa {i}: título CUE inesperado: {item.get('title')!r}")
        range_text = "ausente (arquivo físico inteiro)" if pair is None else (f"{pair[0]:.6f}-{pair[1]:.6f}" if pair[1] is not None else f"{pair[0]:.6f}- (fim aberto)")
        log(f"  {label} faixa {i}: duração efetiva reportada={duration:.6f}s Range={range_text}")

def cue_paths(client, name):
    found = []
    for item in blocks(client.cmd("listallinfo")):
        uri = item.get("file", "")
        if name.casefold() in uri.casefold() and re.search(r"\.cue/track\d+$", uri, re.I):
            found.append(item)
    return sorted(found, key=lambda x: int(re.search(r"track(\d+)$", x["file"], re.I).group(1)))

def wait_cue(client, name, count):
    end = time.monotonic() + 20
    while time.monotonic() < end:
        status = client.cmd("status")
        if any(x.startswith("updating_db: ") for x in status):
            time.sleep(.1)
            continue
        found = cue_paths(client, name)
        if len(found) == count:
            return found
        time.sleep(.2)
    raise Failure(f"Descoberta nativa não encontrou {count} faixas em {name}.")

def wait_id(client, expected, timeout=3):
    deadline, last = time.monotonic() + timeout, {}
    while time.monotonic() < deadline:
        last = dict(x.split(": ", 1) for x in client.cmd("status") if ": " in x)
        if last.get("songid") == str(expected):
            return last
        if last.get("state") == "stop":
            break
        time.sleep(.05)
    raise Failure(f"MPD não confirmou songid={expected}; status={last!r}")

def queue_native(client, entries):
    client.cmd("clear")
    ids = []
    for entry in entries:
        response = client.cmd("addid", ("q", entry["file"]))
        row = next((x for x in response if x.startswith("Id: ")), None)
        if not row:
            raise Failure(f"addid CUE sem ID: {response!r}")
        ids.append(int(row.split(": ", 1)[1]))
    return ids

def queue_range(client, entries):
    client.cmd("clear")
    ids = []
    for entry in entries:
        response = client.cmd("addid", ("q", str(entry["audio"])))
        row = next((x for x in response if x.startswith("Id: ")), None)
        if not row:
            raise Failure(f"addid absoluto recusado: {response!r}")
        song_id = int(row.split(": ", 1)[1])
        if not entry.get("whole_file"):
            start, end = sec_text(cue_seconds(entry["start"])), sec_text(cue_seconds(entry["end"]))
            client.cmd("rangeid", song_id, f"{start}:{end}")
        ids.append(song_id)
    return ids

def exercise(client, ids, expected, label):
    first, second, third = ids
    d1 = float(cue_seconds(expected[0]["end"] - expected[0]["start"]))
    d2 = float(cue_seconds(expected[1]["end"] - expected[1]["start"]))
    began = time.monotonic()
    client.cmd("playid", first)
    wait_id(client, second, d1 + 3)
    observed1 = time.monotonic() - began
    if abs(observed1 - d1) > max(.9, .2 * d1):
        raise Failure(f"{label}: primeira faixa durou {observed1:.3f}s; esperado {d1:.3f}s.")
    client.cmd("currentsong")
    client.cmd("previous")
    wait_id(client, first)
    client.cmd("pause", 1)
    status = dict(x.split(": ", 1) for x in client.cmd("status") if ": " in x)
    if status.get("state") != "pause":
        raise Failure(f"{label}: Pause não refletido: {status!r}")
    client.cmd("seekid", first, "1.000")
    status = dict(x.split(": ", 1) for x in client.cmd("status") if ": " in x)
    pos = number(status.get("elapsed"))
    if pos is None or not .65 <= pos <= 1.35:
        raise Failure(f"{label}: Seek 1s não refletido: {status!r}")
    client.cmd("currentsong")
    client.cmd("pause", 0)
    client.cmd("next")
    wait_id(client, second)
    client.cmd("previous")
    wait_id(client, first)
    client.cmd("next")
    wait_id(client, second)
    client.cmd("seekid", second, "0.000")
    began = time.monotonic()
    deadline = began + d2 + 3
    last = {}
    while time.monotonic() < deadline:
        last = dict(x.split(": ", 1) for x in client.cmd("status") if ": " in x)
        if last.get("songid") == str(third) or last.get("state") == "stop":
            break
        time.sleep(.05)
    observed2 = time.monotonic() - began
    if last.get("songid") != str(third):
        raise Failure(f"{label}: avanço automático não chegou à faixa 3: {last!r}")
    if abs(observed2 - d2) > max(.9, .2 * d2):
        raise Failure(f"{label}: segunda faixa durou {observed2:.3f}s; esperado {d2:.3f}s.")
    client.cmd("currentsong")
    log(f"  {label}: Play/Pause/Seek/Next/Previous e auto-advance OK; durações observadas {observed1:.3f}s/{observed2:.3f}s.")
    client.cmd("stop")

def config_text(root):
    q = lambda x: '"' + str(x).replace("\\", "\\\\").replace('"', '\\"') + '"'
    return "\n".join([
        f"music_directory {q(root / 'music')}", f"playlist_directory {q(root / 'playlists')}",
        f"db_file {q(root / 'database/mpd.db')}", f"state_file {q(root / 'state/state')}",
        f"sticker_file {q(root / 'state/stickers')}", f"pid_file {q(root / 'state/mpd.pid')}",
        f"log_file {q(root / 'logs/mpd.log')}", f"bind_to_address {q(root / 'mpd.sock')}",
        'auto_update "no"',
        'zeroconf_enabled "no"', 'restore_paused "no"',
        "playlist_plugin {", '  name "cue"', '  enabled "yes"', '  as_directory "yes"', "}",
        "audio_output {", '  type "null"', f"  name {q(OUTPUT)}", "}", "",
    ])

def private_binary(path):
    home = Path.home()
    if home.resolve(strict=True) != home:
        raise Failure("HOME contém symlink; runtime não será seguido.")
    base = home / ".local/share/sonante-0.6.4-sqlite/runtime/eq-dev"
    if base.resolve(strict=True) != base:
        raise Failure("O diretório do runtime privado contém symlink.")
    if not path.is_absolute() or path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
        raise Failure("MPD precisa ser arquivo executável absoluto, regular e sem symlink.")
    exe = path.resolve(strict=True)
    if base not in exe.parents or exe.stat().st_uid != os.getuid() or exe.stat().st_mode & 0o022:
        raise Failure("MPD não corresponde a arquivo próprio e privado do runtime EQ.")
    digest = hashlib.sha256()
    with exe.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    if digest.hexdigest() != PIN:
        raise Failure(f"SHA-256 diferente do runtime MPD EQ qualificado: {digest.hexdigest()}.")
    result = subprocess.run([str(exe), "--version"], stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                            timeout=10, env={"PATH": "/usr/bin:/bin", "HOME": str(home), "LC_ALL": "C"})
    if result.returncode or not re.search(r"Music Player Daemon 0\.24\.15(?:\s|\()", result.stdout):
        raise Failure("Runtime privado não confirmou MPD 0.24.15.")
    section = re.search(r"Playlist plugins:\s*(.*?)(?:\n\S.*:|\Z)", result.stdout, re.S)
    if not section or not re.search(r"\bcue\b", section.group(1)):
        raise Failure("Runtime privado não anuncia o plugin CUE.")
    return exe, result.stdout.strip(), digest.hexdigest()

def parent_pid(pid):
    text = Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
    return int(text.rsplit(")", 1)[1].split()[1])

def tcp_listeners_for(process):
    owned_inodes = set()
    for fd in (Path(f"/proc/{process.pid}") / "fd").iterdir():
        try:
            match = re.fullmatch(r"socket:\[(\d+)\]", os.readlink(fd))
            if match:
                owned_inodes.add(match.group(1))
        except OSError:
            continue
    listeners = []
    for table in (Path("/proc/net/tcp"), Path("/proc/net/tcp6")):
        if not table.is_file():
            continue
        for row in table.read_text(encoding="ascii").splitlines()[1:]:
            fields = row.split()
            if len(fields) > 9 and fields[3] == "0A" and fields[9] in owned_inodes:
                listeners.append((table.name, fields[1]))
    return listeners

def owned(process, exe, conf, pidfile, need_pid=True):
    if process.poll() is not None:
        return False
    proc = Path(f"/proc/{process.pid}")
    try:
        if proc.stat().st_uid != os.getuid() or parent_pid(process.pid) != os.getpid():
            return False
        if os.path.realpath(proc / "exe") != str(exe):
            return False
        args = [x.decode("utf-8", "replace") for x in (proc / "cmdline").read_bytes().split(b"\0")]
        if str(conf) not in args:
            return False
        if pidfile.exists():
            return pidfile.is_file() and pidfile.read_text(encoding="ascii").strip() == str(process.pid)
        return not need_pid
    except (OSError, ValueError, IndexError):
        return False

def ownership_problem(process, exe, conf, pidfile):
    if process.poll() is not None:
        return f"child_exited:{process.returncode}"
    proc = Path(f"/proc/{process.pid}")
    try:
        if proc.stat().st_uid != os.getuid():
            return "child_uid_mismatch"
        if parent_pid(process.pid) != os.getpid():
            return "child_parent_pid_mismatch"
        if os.path.realpath(proc / "exe") != str(exe):
            return "child_executable_mismatch"
        args = [x.decode("utf-8", "replace") for x in (proc / "cmdline").read_bytes().split(b"\0")]
        if str(conf) not in args:
            return "child_config_argument_missing"
        info = inode_details(pidfile)
        if not info.get("exists"):
            return "pidfile_absent"
        if info.get("type") != "regular" or info.get("uid") != os.getuid():
            return f"pidfile_type_or_owner_mismatch:{info.get('type')}:{info.get('uid')}"
        if pidfile.read_text(encoding="ascii").strip() != str(process.pid):
            return "pidfile_pid_mismatch"
    except (OSError, ValueError, IndexError) as exc:
        return f"ownership_check_error:{exc}"
    return None

def kind(mode):
    if stat.S_ISSOCK(mode): return "socket"
    if stat.S_ISDIR(mode): return "directory"
    if stat.S_ISREG(mode): return "regular"
    if stat.S_ISLNK(mode): return "symlink"
    if stat.S_ISFIFO(mode): return "fifo"
    if stat.S_ISCHR(mode): return "character-device"
    if stat.S_ISBLK(mode): return "block-device"
    return "other"

def inode_details(path):
    try:
        st = path.lstat()
    except FileNotFoundError:
        return {"exists": False, "path": str(path)}
    except OSError as exc:
        return {"exists": "unknown", "path": str(path), "error": str(exc)}
    return {
        "exists": True, "path": str(path), "type": kind(st.st_mode),
        "uid": st.st_uid, "mode": f"{stat.S_IMODE(st.st_mode):04o}",
        "device": st.st_dev, "inode": st.st_ino,
    }

def describe_inode(label, path, expected_uid):
    info = inode_details(path)
    if not info.get("exists"):
        return f"{label}: exists={info.get('exists')} path={path} error={info.get('error', '-') }"
    return (f"{label}: exists=yes type={info['type']} uid={info['uid']} expected_uid={expected_uid} "
            f"mode={info['mode']} dev={info['device']} inode={info['inode']} path={path}")

def private_layout(root):
    problems = []
    paths = [root] + [root / name for name in (
        "music", "direct-only", "playlists", "database", "state", "logs",
        "home", "config", "data", "cache", "xdg-state", "runtime", "tmp",
    )]
    expected_uid = os.getuid()
    for path in paths:
        info = inode_details(path)
        if not info.get("exists") or info.get("type") != "directory":
            problems.append(f"{path}: não é diretório existente (type={info.get('type', '-')}).")
        elif info["uid"] != expected_uid or int(info["mode"], 8) & 0o077:
            problems.append(f"{path}: diretório não privado (uid={info['uid']}, mode={info['mode']}).")
    return problems

def report_startup(label, root, process, exe, conf, pidfile, sockpath, began, notes, condition=None):
    expected_uid = os.getuid()
    lines = [f"startup.{label}.elapsed_seconds={time.monotonic() - began:.3f}"]
    if condition:
        lines.append(f"startup.{label}.condition={condition}")
    if process is None:
        lines.append("startup.process=not-started")
    else:
        poll = process.poll()
        proc_stat = Path(f"/proc/{process.pid}/stat")
        try:
            state = proc_stat.read_text(encoding="ascii").rsplit(")", 1)[1].split()[0]
        except (OSError, IndexError):
            state = "unavailable"
        lines.append(f"startup.process.pid={process.pid} running={poll is None} exit={poll} state={state}")
        lines.append(f"startup.process.owned={owned(process, exe, conf, pidfile, need_pid=False)}")
        lines.append(f"startup.process.ownership_problem={ownership_problem(process, exe, conf, pidfile)}")
    lines.append(describe_inode("socket", sockpath, expected_uid))
    lines.append(describe_inode("socket_parent", sockpath.parent, expected_uid))
    lines.append(describe_inode("temporary_parent", root.parent, expected_uid))
    lines.append(describe_inode("pid_file", pidfile, expected_uid))
    lines.append(describe_inode("database_file", root / "database/mpd.db", expected_uid))
    lines.append(describe_inode("state_file", root / "state/state", expected_uid))
    lines.append(describe_inode("sticker_file", root / "state/stickers", expected_uid))
    lines.append(describe_inode("config_file", conf, expected_uid))
    lines.append(describe_inode("log_file", root / "logs/mpd.log", expected_uid))
    lines.append(describe_inode("database_directory", root / "database", expected_uid))
    for item in [root, root / "state", root / "logs", root / "playlists", root / "music",
                 root / "direct-only", root / "home", root / "config", root / "data",
                 root / "cache", root / "xdg-state", root / "runtime", root / "tmp"]:
        lines.append(describe_inode("directory", item, expected_uid))
    for line in lines:
        notes.append(line)
        log(line)

def normalize_socket_mode(root, sockpath, process, exe, conf, pidfile, expected_inode):
    if sockpath.parent != root or root.resolve(strict=True) != root:
        raise Failure("socket_parent_not_exact_private_root")
    if not owned(process, exe, conf, pidfile):
        raise Failure("child_ownership_not_confirmed_before_socket_chmod")
    problems = private_layout(root)
    if problems:
        raise Failure("private_directory_validation_failed_before_socket_chmod: " + " ".join(problems))
    if not hasattr(os, "O_NOFOLLOW") or os.chmod not in os.supports_dir_fd:
        raise Failure("safe_dirfd_socket_chmod_unavailable")
    dirfd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        root_stat = os.fstat(dirfd)
        path_root_stat = root.lstat()
        if ((root_stat.st_dev, root_stat.st_ino) != (path_root_stat.st_dev, path_root_stat.st_ino)
                or not stat.S_ISDIR(root_stat.st_mode) or root_stat.st_uid != os.getuid()
                or stat.S_IMODE(root_stat.st_mode) & 0o077):
            raise Failure("private_socket_root_changed_before_chmod")
        current = os.stat(sockpath.name, dir_fd=dirfd, follow_symlinks=False)
        if (not stat.S_ISSOCK(current.st_mode) or current.st_uid != os.getuid()
                or (current.st_dev, current.st_ino) != expected_inode):
            raise Failure("socket_identity_changed_before_chmod")
        os.chmod(sockpath.name, 0o600, dir_fd=dirfd)
        after = os.stat(sockpath.name, dir_fd=dirfd, follow_symlinks=False)
        if ((after.st_dev, after.st_ino) != expected_inode or not stat.S_ISSOCK(after.st_mode)
                or after.st_uid != os.getuid() or stat.S_IMODE(after.st_mode) != 0o600):
            raise Failure("socket_revalidation_failed_after_chmod")
    finally:
        os.close(dirfd)
    if not owned(process, exe, conf, pidfile):
        raise Failure("child_ownership_changed_after_socket_chmod")

def wait_for_private_socket(root, process, exe, conf, pidfile, sockpath, began, notes):
    deadline = began + 15
    expected_uid = os.getuid()
    while time.monotonic() < deadline:
        exit_code = process.poll()
        if exit_code is not None:
            report_startup("child-exited", root, process, exe, conf, pidfile, sockpath, began, notes,
                           f"mpd_child_exited_before_socket:exit={exit_code}")
            raise Failure(f"mpd_child_exited_before_socket:exit={exit_code}")
        info = inode_details(sockpath)
        if info.get("exists") == "unknown":
            condition = f"socket_lstat_failed:{info.get('error')}"
            report_startup("lstat-failed", root, process, exe, conf, pidfile, sockpath, began, notes, condition)
            raise Failure(condition)
        if info.get("exists") and info.get("type") != "socket":
            condition = f"socket_wrong_type:{info.get('type')}"
            report_startup("wrong-type", root, process, exe, conf, pidfile, sockpath, began, notes, condition)
            raise Failure(condition)
        if info.get("type") == "socket":
            parent = inode_details(sockpath.parent)
            if parent.get("type") != "directory" or parent.get("uid") != expected_uid or int(parent.get("mode", "777"), 8) & 0o077:
                condition = (f"socket_parent_not_private:type={parent.get('type')} uid={parent.get('uid')} "
                             f"expected_uid={expected_uid} mode={parent.get('mode')}")
                report_startup("unsafe-parent", root, process, exe, conf, pidfile, sockpath, began, notes, condition)
                raise Failure(condition)
            if info.get("uid") != expected_uid:
                condition = f"socket_owner_mismatch:uid={info.get('uid')}:expected={expected_uid}"
                report_startup("wrong-owner", root, process, exe, conf, pidfile, sockpath, began, notes, condition)
                raise Failure(condition)
            if pidfile.parent != root / "state":
                condition = "pidfile_parent_not_expected_private_state_directory"
                report_startup("pidfile-parent-invalid", root, process, exe, conf, pidfile,
                               sockpath, began, notes, condition)
                raise Failure(condition)
            pid_info = inode_details(pidfile)
            if pid_info.get("exists") and (pid_info.get("type") != "regular" or pid_info.get("uid") != expected_uid):
                condition = f"pidfile_invalid:type={pid_info.get('type')}:uid={pid_info.get('uid')}:expected={expected_uid}"
                report_startup("pidfile-invalid", root, process, exe, conf, pidfile, sockpath, began, notes, condition)
                raise Failure(condition)
            if pid_info.get("type") == "regular" and owned(process, exe, conf, pidfile):
                inode = (info["device"], info["inode"])
                if stat.S_IMODE(int(info["mode"], 8)) != 0o600:
                    condition = f"socket_mode_normalization:observed={info['mode']}:target=0600"
                    report_startup("before-normalization", root, process, exe, conf, pidfile, sockpath,
                                   began, notes, condition)
                    try:
                        normalize_socket_mode(root, sockpath, process, exe, conf, pidfile, inode)
                    except Exception as exc:
                        report_startup("normalization-failed", root, process, exe, conf, pidfile,
                                       sockpath, began, notes, str(exc))
                        raise Failure(f"socket_mode_normalization_failed:{exc}") from exc
                time.sleep(.10)
                after = inode_details(sockpath)
                final_parent = inode_details(sockpath.parent)
                layout_problems = private_layout(root)
                if (after.get("type") != "socket" or after.get("uid") != expected_uid
                        or (after.get("device"), after.get("inode")) != inode
                        or after.get("mode") != "0600" or final_parent.get("type") != "directory"
                        or final_parent.get("uid") != expected_uid
                        or int(final_parent.get("mode", "777"), 8) & 0o077
                        or layout_problems or not owned(process, exe, conf, pidfile)):
                    condition = (f"socket_revalidation_failed:type={after.get('type')}:uid={after.get('uid')}:"
                                 f"mode={after.get('mode')}:inode_same="
                                 f"{(after.get('device'), after.get('inode')) == inode}:"
                                 f"parent_mode={final_parent.get('mode')}:layout={layout_problems}")
                    report_startup("revalidation-failed", root, process, exe, conf, pidfile,
                                   sockpath, began, notes, condition)
                    raise Failure(condition)
                report_startup("ready", root, process, exe, conf, pidfile, sockpath, began, notes)
                return
        time.sleep(.05)
    info = inode_details(sockpath)
    pid_info = inode_details(pidfile)
    if not info.get("exists"):
        condition = "timeout_socket_absent"
    elif not pid_info.get("exists"):
        condition = "timeout_pidfile_absent"
    elif pid_info.get("type") != "regular" or pid_info.get("uid") != expected_uid:
        condition = f"timeout_pidfile_invalid:type={pid_info.get('type')}:uid={pid_info.get('uid')}"
    else:
        problem = ownership_problem(process, exe, conf, pidfile)
        condition = f"timeout_child_ownership_mismatch:{problem}" if problem else "timeout_socket_not_ready"
    report_startup("timeout", root, process, exe, conf, pidfile, sockpath, began, notes, condition)
    raise Failure(condition)

def evidence_dir(path):
    if not path:
        return Path(tempfile.mkdtemp(prefix="sonante-cue-evidence.", dir="/tmp"))
    out = Path(path).expanduser()
    if not out.is_absolute() or not out.is_dir() or out.is_symlink() or any(out.iterdir()):
        raise Failure("Use --evidence-dir com um diretório novo, vazio e absoluto.")
    if out.parent.resolve(strict=True) != out.parent.absolute():
        raise Failure("O caminho das evidências contém symlink.")
    if out.stat().st_uid != os.getuid() or stat.S_IMODE(out.stat().st_mode) & 0o077:
        raise Failure("Diretório de evidências deve pertencer ao usuário e ter modo 0700.")
    return out.resolve(strict=True)

def main():
    parser = argparse.ArgumentParser(description="Prova CUE isolada: MPD 0.24.15, socket Unix e saída null.")
    parser.add_argument("--mpd", type=Path, default=DEFAULT_MPD)
    parser.add_argument("--evidence-dir")
    parser.add_argument("--self-check", action="store_true", help="validar fixtures/config sem iniciar MPD")
    args = parser.parse_args()
    args.mpd = args.mpd.expanduser()
    if args.self_check:
        self_check()
        return 0

    root = None
    proc = None
    client = None
    transcript = None
    results = []
    notes = []
    evidence = None
    unsafe = False
    try:
        evidence = evidence_dir(args.evidence_dir)
        exe, version, sha = private_binary(args.mpd)
        notes.append(f"Runtime MPD 0.24.15 verificado; SHA-256={sha}; plugin CUE presente.")
        log(f"Runtime verificado: MPD 0.24.15, SHA-256 {sha}, plugin CUE presente.")
        root = Path(tempfile.mkdtemp(prefix="scue.", dir="/tmp")).resolve(strict=True)
        os.chmod(root, 0o700)
        for name in ["music", "direct-only", "playlists", "database", "state", "logs",
                     "home", "config", "data", "cache", "xdg-state", "runtime", "tmp"]:
            (root / name).mkdir(mode=0o700)
        music, direct = root / "music", root / "direct-only"
        (single_cue, single, direct_single), (multi_cue, multi, direct_multi) = cue_fixtures(music, direct)
        conf = root / "mpd.conf"
        conf.write_text(config_text(root), encoding="utf-8")
        os.chmod(conf, 0o600)
        protocol = root / "protocol.log"
        transcript = protocol.open("w", encoding="utf-8")
        child_env = {
            "PATH": "/usr/bin:/bin", "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
            "XDG_CACHE_HOME": str(root / "cache"), "XDG_STATE_HOME": str(root / "xdg-state"),
            "XDG_RUNTIME_DIR": str(root / "runtime"), "TMPDIR": str(root / "tmp"), "LC_ALL": "C",
        }
        pidfile, sockpath = root / "state/mpd.pid", root / "mpd.sock"
        began = time.monotonic()
        layout_problems = private_layout(root)
        if layout_problems:
            report_startup("preflight-failed", root, None, exe, conf, pidfile, sockpath,
                           began, notes, "private_layout_invalid: " + " ".join(layout_problems))
            raise Failure("private_layout_invalid: " + " ".join(layout_problems))
        report_startup("pre-launch", root, None, exe, conf, pidfile, sockpath, began, notes)
        with (root / "logs/mpd.log").open("wb") as mpdlog:
            proc = subprocess.Popen([str(exe), "--no-daemon", str(conf)], stdin=subprocess.DEVNULL,
                                    stdout=mpdlog, stderr=subprocess.STDOUT, cwd=root, env=child_env,
                                    close_fds=True, umask=0o077)
            wait_for_private_socket(root, proc, exe, conf, pidfile, sockpath, began, notes)
            listeners = tcp_listeners_for(proc)
            if listeners:
                raise Failure(f"MPD abriu listener TCP inesperado: {listeners!r}")
            client = Client(sockpath, transcript)
            output = blocks(client.cmd("outputs"), "outputid")
            if len(output) != 1 or output[0].get("outputname") != OUTPUT or output[0].get("outputenabled") != "1":
                raise Failure(f"MPD não confirmou a única saída null ativa: {output!r}")
            client.cmd("update", ("q", ""))
            native_ok = False
            range_ok = False
            try:
                log("Teste nativo: descoberta CUE, metadados, ranges, durações, controles e transições.")
                entries = wait_cue(client, "single-image.cue", 3)
                report_startup("after-index-update", root, proc, exe, conf, pidfile,
                               sockpath, began, notes)
                ids = queue_native(client, entries)
                check_items(client, ids, single, "CUE nativo", cue_metadata=True)
                exercise(client, ids, single, "CUE nativo")
                native_ok = True
                results.append("CUE nativo single-file: PASS")
            except Exception as exc:
                log(f"  FALHA: {exc}")
                results.append(f"CUE nativo single-file: FAIL — {exc}")

            try:
                log("Teste rangeid: addid absoluto de WAV fora de music_directory.")
                ids = queue_range(client, direct_single)
                check_items(client, ids, direct_single, "addid+rangeid", direct=True)
                client.cmd("status")
                client.cmd("currentsong")
                client.cmd("playlistinfo")
                try:
                    client.cmd("save", ("q", "cue-rangeid-roundtrip"))
                    client.cmd("listplaylistinfo", ("q", "cue-rangeid-roundtrip"))
                    client.cmd("clear")
                    client.cmd("load", ("q", "cue-rangeid-roundtrip"))
                    restored = blocks(client.cmd("playlistinfo"))
                    ranges = [x.get("range") for x in restored]
                    note = f"MPD save/load: {len(restored)} itens; Range restaurado={ranges!r}"
                except Exception as exc:
                    note = f"MPD save/load indisponível: {exc}"
                notes.append(note)
                log(f"  {note}")
                ids = queue_range(client, direct_single)
                exercise(client, ids, direct_single, "addid+rangeid")
                range_ok = True
                results.append("addid absoluto + rangeid single-file: PASS")
            except Exception as exc:
                log(f"  FALHA: {exc}")
                results.append(f"addid absoluto + rangeid single-file: FAIL — {exc}")

            if native_ok and range_ok:
                try:
                    log("Teste multiarquivo: CUE nativo e referências rangeid por arquivo.")
                    entries = wait_cue(client, "multi-image.cue", 3)
                    ids = queue_native(client, entries)
                    check_items(client, ids, multi, "CUE multiarquivo", cue_metadata=True)
                    exercise(client, ids, multi, "CUE nativo multiarquivo")
                    ids = queue_range(client, direct_multi)
                    check_items(client, ids, direct_multi, "rangeid multiarquivo", direct=True)
                    exercise(client, ids, direct_multi, "addid+rangeid multiarquivo")
                    results.append("CUE nativo e addid+rangeid multiarquivo (controles/transições): PASS")
                except Exception as exc:
                    log(f"  FALHA: {exc}")
                    results.append(f"CUE multiarquivo: FAIL — {exc}")
            else:
                notes.append("Multiarquivo omitido porque uma das provas básicas falhou.")
            client.cmd("stop")
        if transcript:
            transcript.flush()
            transcript.close()
            transcript = None
    except KeyboardInterrupt:
        results.append("Interrompido pelo usuário.")
        log("Interrompido; encerrando somente o filho próprio após verificar ownership.")
    except Exception as exc:
        results.append(f"FAIL — preparação/startup: {exc}")
        log(f"FALHA: {exc}")
    finally:
        if client:
            try:
                client.close()
            except Exception:
                pass
        if transcript and not transcript.closed:
            transcript.close()
        if proc and proc.poll() is None and root:
            conf = root / "mpd.conf"
            pidfile = root / "state/mpd.pid"
            try:
                exe = args.mpd.expanduser().resolve(strict=True)
            except OSError:
                exe = Path("/nonexistent")
            if owned(proc, exe, conf, pidfile, need_pid=False):
                proc.send_signal(signal.SIGTERM)
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    if owned(proc, exe, conf, pidfile, need_pid=False):
                        proc.kill()
                        proc.wait(timeout=3)
                    else:
                        unsafe = True
            else:
                unsafe = True
                log("SEGURANÇA: ownership não confirmado; nenhum sinal foi enviado.")
        if evidence:
            if root:
                for name, source in [("mpd.conf", root / "mpd.conf"),
                                     ("protocol.log", root / "protocol.log"),
                                     ("mpd.log", root / "logs/mpd.log")]:
                    if source.is_file():
                        shutil.copyfile(source, evidence / name)
                        os.chmod(evidence / name, 0o600)
            summary = "\n".join(results + notes + [f"Cleanup de processo seguro: {not unsafe}"]) + "\n"
            (evidence / "summary.txt").write_text(summary, encoding="utf-8")
            os.chmod(evidence / "summary.txt", 0o600)
            log(f"Evidências: {evidence}")
        if root:
            stopped = proc is None or proc.poll() is not None
            if stopped and not unsafe and root.parent == Path("/tmp") and root.name.startswith("scue.") and root.lstat().st_uid == os.getuid() and not root.is_symlink():
                shutil.rmtree(root)
            else:
                log(f"Dados temporários preservados por segurança: {root}")
    if unsafe or not results or any("FAIL" in x or "Interrompido" in x for x in results):
        return 1
    return 0

if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (Failure, OSError) as exc:
        log(f"ERRO: {exc}")
        raise SystemExit(2)
PYTHON
