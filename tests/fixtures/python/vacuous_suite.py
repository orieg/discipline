import unittest

def test_empty():
    pass

def test_tautology_bool():
    assert True

def test_tautology_math():
    assert 1 == 1

class VacuousTest(unittest.TestCase):
    def test_tautology_assert_equal(self):
        self.assertEqual(1, 1)

    def test_tautology_assert_true(self):
        self.assertTrue(True)
