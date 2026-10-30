#!/usr/bin/env python3
"""Fail when a GNU release archive needs a newer glibc than it declares.

Usage: check-glibc-min.py MANIFEST ARCHIVE...

MANIFEST is the packslip release.toml. For every `*-linux-gnu*` archive, the
`jouet` executable inside is read with `readelf -V`, which works for any ELF
architecture, and the highest `GLIBC_x.y` symbol version it needs is compared
with the archive's `requires.glibc_min`. An archive the manifest gives no
`glibc_min`, or one whose binary needs more, fails, so the declared floor and
the real one cannot drift apart.
"""

import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path


def version(text: str) -> tuple[int, ...]:
    return tuple(int(part) for part in text.split("."))


def needed(archive: Path, binary: str = "jouet") -> tuple[int, ...]:
    with tarfile.open(archive) as tar, tempfile.TemporaryDirectory() as tmp:
        member = next(
            (m for m in tar.getmembers() if m.isfile() and Path(m.name).name == binary),
            None,
        )
        if member is None:
            raise SystemExit(f"::error::{archive.name} holds no {binary} executable")
        tar.extract(member, tmp, filter="data")
        out = subprocess.run(
            ["readelf", "-V", "-W", str(Path(tmp) / member.name)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    return max((version(v) for v in re.findall(r"Name: GLIBC_([0-9.]+)", out)), default=())


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    manifest = tomllib.loads(Path(sys.argv[1]).read_text())
    declared = {
        Path(a["path"]).name: a.get("requires", {}).get("glibc_min")
        for a in manifest.get("artifact", [])
    }
    status = 0
    checked = 0
    for archive in map(Path, sys.argv[2:]):
        if "-linux-gnu" not in archive.name:
            continue
        checked += 1
        floor = declared.get(archive.name)
        if not floor:
            print(f"::error::{archive.name} has no requires.glibc_min in {sys.argv[1]}")
            status = 1
            continue
        need = needed(archive)
        shown = ".".join(map(str, need)) or "none"
        if need > version(floor):
            print(f"::error::{archive.name} needs glibc {shown}, but {sys.argv[1]} declares {floor}")
            status = 1
        else:
            print(f"{archive.name}: needs glibc {shown}, declares {floor}")
    if checked == 0:
        print("::error::no *-linux-gnu archive was given")
        status = 1
    return status


if __name__ == "__main__":
    sys.exit(main())
