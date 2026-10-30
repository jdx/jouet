import unittest
from decimal import Decimal

from invoice import average_line

LINES = [("widget", 3, "2.50"), ("gadget", 1, "0.125")]


class AverageTest(unittest.TestCase):
    def test_average(self):
        self.assertEqual(average_line(LINES), Decimal("3.82"))
