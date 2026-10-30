from calc.core import arith


def test_add(two):
    assert arith.add(two, 1) == 3
