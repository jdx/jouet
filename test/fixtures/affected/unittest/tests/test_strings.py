import unittest

from calc.strings import shout


class StringsTest(unittest.TestCase):
    def test_shout(self):
        self.assertEqual(shout("hi"), "HI")
