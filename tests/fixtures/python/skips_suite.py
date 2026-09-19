import unittest
import pytest

@pytest.mark.skip(reason="standalone skip")
def test_skipped_fn():
    assert 1 == 2

@unittest.skip("function skip")
def test_unittest_skipped_fn():
    assert 1 == 2

@unittest.skip("class skipped")
class SkippedSuite(unittest.TestCase):
    def test_in_skipped_suite(self):
        assert 1 == 2

class MarkSkippedSuite(unittest.TestCase):
    pytestmark = pytest.mark.skip("class pytestmark skip")

    def test_method_skip(self):
        assert 1 == 2
