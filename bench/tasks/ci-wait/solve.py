import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

p = Path("amounts.py")
old = '    text = text.strip().replace(",", "")\n'
new = old + '    if text.startswith("(") and text.endswith(")"):\n        return -parse_amount(text[1:-1])\n'
assert old in p.read_text()
p.write_text(p.read_text().replace(old, new))
commit("fix: parse accounting negatives again", "amounts.py")
