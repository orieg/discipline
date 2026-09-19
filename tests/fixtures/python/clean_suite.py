import unittest
import pytest

def test_standalone_math():
    x = 10 + 20
    assert x == 30
    assert x > 0

class CalculatorTest(unittest.TestCase):
    def test_addition(self):
        self.assertEqual(1 + 1, 2)
        self.assertTrue(2 > 0)

    def test_subtraction(self):
        self.assertEqual(5 - 3, 2)
