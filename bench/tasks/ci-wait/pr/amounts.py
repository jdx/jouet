"""Parsing money amounts from ledger exports."""


def parse_amount(text):
    """Parse '1,234.50', '-12.50' or '(12.50)', an accounting negative, to a float."""
    text = text.strip().replace(",", "")
    negative = text.startswith("-")
    value = float(text.lstrip("-"))
    return -value if negative else value
