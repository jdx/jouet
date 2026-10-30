import unittest
from decimal import Decimal

from invoice import total

LINES = [("widget", 3, "2.50"), ("gadget", 1, "0.125")]


class TotalTest(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total(LINES), Decimal("7.63"))

    def test_empty(self):
        self.assertEqual(total([]), Decimal("0.00"))
