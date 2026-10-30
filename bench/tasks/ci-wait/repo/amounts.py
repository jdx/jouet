"""Parsing money amounts from ledger exports."""


def parse_amount(text):
    """Parse '12.50', '-12.50' or '(12.50)', an accounting negative, to a float."""
    text = text.strip()
    if text.startswith("(") and text.endswith(")"):
        return -parse_amount(text[1:-1])
    return float(text)
