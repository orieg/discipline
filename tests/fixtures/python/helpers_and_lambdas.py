def assert_positive(val):
    assert val > 0

def test_with_helpers():
    check_value = lambda x: assert_positive(x)
    assert_positive(42)
