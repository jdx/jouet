import os
import unittest
from pathlib import Path

from amounts import parse_amount


class AmountTests(unittest.TestCase):
    def test_plain(self):
        self.assertEqual(parse_amount("12.50"), 12.5)

    def test_minus(self):
        self.assertEqual(parse_amount(" -3 "), -3.0)

    @unittest.skipUnless(os.environ.get("CI"), "the ledger corpus runs in CI only")
    def test_ledger_corpus(self):
        for line in Path(__file__).with_name("ledger.txt").read_text().splitlines():
            text, want = line.rsplit(" ", 1)
            with self.subTest(text=text):
                try:
                    got = parse_amount(text)
                except ValueError as e:
                    self.fail(f"parse_amount({text!r}) raised ValueError: {e}")
                self.assertEqual(got, float(want))
