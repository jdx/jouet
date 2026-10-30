import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

paths = [
    "README.md",
    "shipit/config.py",
    "shipit/upload.py",
    "shipit/client.py",
    "shipit/cli.py",
    "tests/test_config.py",
    "tests/test_client.py",
]
for path in paths:
    p = Path(path)
    p.write_text(p.read_text().replace("retry_limit", "max_attempts").replace("retry-limit", "max-attempts"))
commit("refactor: rename retry_limit to max_attempts", *paths)
