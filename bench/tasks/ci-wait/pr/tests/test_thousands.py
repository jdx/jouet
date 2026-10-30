import unittest

from amounts import parse_amount


class ThousandsTests(unittest.TestCase):
    def test_separators(self):
        self.assertEqual(parse_amount("1,234.50"), 1234.5)

    def test_negative_separators(self):
        self.assertEqual(parse_amount("-12,000"), -12000.0)
