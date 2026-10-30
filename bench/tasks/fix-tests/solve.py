import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

p = Path("textstats/__init__.py")
text = p.read_text()
for old, new in (
    ("for word in words(text):\n", "for word in words(text.lower()):\n"),
    (
        "key=lambda item: (item[1], item[0]), reverse=True)",
        "key=lambda item: (-item[1], item[0]))",
    ),
    (
        "return len(words(text)) // words_per_minute",
        "return math.ceil(len(words(text)) / words_per_minute)",
    ),
    ('re.findall(r"[.!?]", text)', 're.findall(r"[.!?]+", text)'),
):
    assert old in text, old
    text = text.replace(old, new)
p.write_text(text)
commit("fix: textstats case, ties, rounding and sentence ends", "textstats/__init__.py")
