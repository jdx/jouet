import unittest

from calc.arith import add


class ArithTest(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(1, 2), 3)
