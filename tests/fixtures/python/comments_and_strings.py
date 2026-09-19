def test_comments_and_strings():
    # assert 1 == 2
    # self.assertEqual(1, 2)
    s = "assert 1 == 2"
    doc = """
    with pytest.raises(ValueError):
        pass
    """
    assert len(s) == 13
