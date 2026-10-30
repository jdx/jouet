import unittest

import textstats as ts


class WordsTest(unittest.TestCase):
    def test_words(self):
        self.assertEqual(ts.words("It's a dog's life."), ["It's", "a", "dog's", "life"])

    def test_frequencies_ignore_case(self):
        self.assertEqual(ts.word_frequencies("The cat. the CAT, a dog"), {"the": 2, "cat": 2, "a": 1, "dog": 1})


class TopWordsTest(unittest.TestCase):
    def test_most_frequent_first(self):
        self.assertEqual(ts.top_words("b b b a a c", 2), ["b", "a"])

    def test_ties_alphabetical(self):
        self.assertEqual(ts.top_words("pear apple fig apple pear fig", 3), ["apple", "fig", "pear"])


class ReadingTimeTest(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(ts.reading_time(""), 0)

    def test_rounds_up(self):
        self.assertEqual(ts.reading_time("word " * 201), 2)

    def test_short_text_takes_a_minute(self):
        self.assertEqual(ts.reading_time("just a few words"), 1)


class SentencesTest(unittest.TestCase):
    def test_simple(self):
        self.assertEqual(ts.sentence_count("One. Two! Three?"), 3)

    def test_ellipsis_is_one(self):
        self.assertEqual(ts.sentence_count("Wait... What?!"), 2)


class SummaryTest(unittest.TestCase):
    def test_summary(self):
        self.assertEqual(
            ts.summary("A cat sat. A cat ran!"),
            {"words": 6, "sentences": 2, "minutes": 1, "top": ["a", "cat", "ran"]},
        )
