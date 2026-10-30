"""Invoices: lines of (description, quantity, unit price)."""

from decimal import ROUND_HALF_UP, Decimal


def line_total(line):
    _, quantity, unit_price = line
    return Decimal(str(quantity)) * Decimal(str(unit_price))


def total(lines):
    """The invoice total, rounded to cents."""
    tmp = sum((line_total(line) for line in lines), Decimal("0"))
    return tmp.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)


def average_line(lines):
    """The average line total, rounded to cents."""
    return (total(lines) / len(lines)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)
