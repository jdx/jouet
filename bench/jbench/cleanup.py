"""Remove disposable benchmark trees, including test-created locked directories."""

import shutil
from pathlib import Path
from typing import Any


def remove(path: Path) -> None:
    """Remove a tree, including directories without read or search permission."""

    def unlock(_func: Any, failed: str, _exc: BaseException) -> None:
        for parent in (Path(failed).parent, Path(failed)):
            try:
                parent.chmod(0o700)
            except OSError:
                pass
        if Path(failed).is_dir():
            shutil.rmtree(failed, onexc=unlock)
        else:
            Path(failed).unlink(missing_ok=True)

    if path.exists():
        shutil.rmtree(path, onexc=unlock)
