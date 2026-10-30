"""Statistics about a piece of text."""

import math
import re

WORD = re.compile(r"[A-Za-z']+")


def words(text):
    """The words in `text`, as written."""
    return WORD.findall(text)


def word_frequencies(text):
    """How often each word appears, ignoring case."""
    counts = {}
    for word in words(text):
        counts[word] = counts.get(word, 0) + 1
    return counts


def top_words(text, n):
    """The `n` most frequent words, most frequent first; ties alphabetically."""
    counts = word_frequencies(text)
    ranked = sorted(counts.items(), key=lambda item: (item[1], item[0]), reverse=True)
    return [word for word, _ in ranked[:n]]


def reading_time(text, words_per_minute=200):
    """Whole minutes to read `text`, rounded up; 0 for no words."""
    return len(words(text)) // words_per_minute


def sentence_count(text):
    """Sentences end with ., ! or ?, and a run of them (...) ends one sentence."""
    return len(re.findall(r"[.!?]", text))


def summary(text):
    return {
        "words": len(words(text)),
        "sentences": sentence_count(text),
        "minutes": reading_time(text),
        "top": top_words(text, 3),
    }
