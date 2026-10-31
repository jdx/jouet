"""Lending a run the harness's credentials file, and keeping a refresh.

A run gets a copy of the user's credentials file in its throwaway config
directory. A link is not enough: Claude Code and Codex replace the file when
they refresh a token, which turns the link into a file of its own, and the
run directory is deleted afterwards. A refresh spends the old refresh token,
so losing the new file logs the user out of their own CLI.

After the run, a copy that changed is written back over the user's file,
atomically and under a lock. If the user's file changed too (their own CLI,
or another suite, refreshed meanwhile), the newer of the two is kept, since
the older one's refresh token has been spent.
"""

from __future__ import annotations

import fcntl
import hashlib
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Lent:
    """A credentials file copied into a run."""

    source: Path
    copy: Path
    # The source's contents when they were copied.
    digest: str


def _digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def lend(sources: list[Path], config_dir: Path) -> list[Lent]:
    """Copies each existing source into `config_dir`, readable by the user only."""
    lent = []
    for source in sources:
        if not source.exists():
            continue
        real = source.resolve()
        data = real.read_bytes()
        copy = config_dir / source.name
        fd = os.open(copy, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        lent.append(Lent(source=real, copy=copy, digest=_digest(data)))
    return lent


def give_back(lent: list[Lent]) -> list[str]:
    """Writes refreshed copies back to their sources; returns what it did."""
    notes = []
    for item in lent:
        try:
            data = item.copy.read_bytes()
        except FileNotFoundError:
            continue
        if _digest(data) == item.digest or not data.strip():
            continue
        with _lock(item.source):
            try:
                current = item.source.read_bytes()
            except FileNotFoundError:
                current = None
            if current is not None and _digest(current) != item.digest:
                # Both refreshed. The later write holds the live refresh token.
                if item.source.stat().st_mtime_ns >= item.copy.stat().st_mtime_ns:
                    notes.append(f"kept {item.source}: it changed during the run and is newer than the run's copy")
                    continue
                notes.append(f"replaced {item.source}, which changed during the run, with the run's newer refresh")
            else:
                notes.append(f"wrote the run's refreshed credentials back to {item.source}")
            _replace(item.source, data)
    return notes


def _replace(path: Path, data: bytes) -> None:
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


class _lock:
    """An exclusive lock per credentials file, outside the user's config directory."""

    def __init__(self, source: Path) -> None:
        key = hashlib.sha256(str(source).encode()).hexdigest()[:16]
        self.path = Path(tempfile.gettempdir()) / f"jbench-credentials-{os.getuid()}-{key}.lock"

    def __enter__(self) -> None:
        self.fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(self.fd, fcntl.LOCK_EX)

    def __exit__(self, *exc: object) -> None:
        fcntl.flock(self.fd, fcntl.LOCK_UN)
        os.close(self.fd)
