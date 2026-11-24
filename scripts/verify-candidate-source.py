#!/usr/bin/env python3
"""Require package jobs to build one clean, immutable Sonante 0.7.2 commit."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


def git(root: Path, *args: str) -> str:
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def verify(root: Path, candidate_sha: str, event_sha: str) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", candidate_sha):
        raise ValueError("candidate_sha must be a full lowercase 40-character commit SHA")
    if not re.fullmatch(r"[0-9a-f]{40}", event_sha):
        raise ValueError("workflow event SHA is not a full commit SHA")
    if git(root, "rev-parse", "HEAD") != candidate_sha:
        raise ValueError("checkout HEAD does not equal the requested candidate SHA")
    subprocess.run(["git", "-C", str(root), "merge-base", "--is-ancestor", candidate_sha, event_sha], check=True)
    if git(root, "status", "--porcelain=v1", "--untracked-files=normal"):
        raise ValueError("candidate checkout is dirty; refusing incomplete source")
    version = json.loads((root / "package.json").read_text())["version"]
    if version != "0.7.2":
        raise ValueError(f"candidate commit has unexpected product version: {version}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--event-sha", required=True)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    args = parser.parse_args()
    try:
        verify(args.repo.resolve(), args.candidate_sha, args.event_sha)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Candidate source rejected: {error}", file=sys.stderr)
        return 1
    print(f"Verified clean Sonante 0.7.2 source commit: {args.candidate_sha}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
