import unittest
import pytest

def test_pytest_raises():
    with pytest.raises(ValueError):
        int("not_a_number")

def test_pytest_warns():
    with pytest.warns(UserWarning):
        import warnings
        warnings.warn("test warning", UserWarning)

class ContextManagerAssertions(unittest.TestCase):
    def test_assert_raises(self):
        with self.assertRaises(ZeroDivisionError):
            _ = 1 / 0

    def test_assert_raises_regex(self):
        with self.assertRaisesRegex(ValueError, r"literal"):
            int("abc")

    def test_assert_logs(self):
        with self.assertLogs("app", level="INFO"):
            import logging
            logging.getLogger("app").info("hello")
