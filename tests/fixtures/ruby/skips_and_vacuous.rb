require 'minitest/autorun'

class SkipsAndVacuousTest < Minitest::Test
  def test_skipped_suite
    skip "Temporarily skipped in CI"
    assert_equal 1, 1
  end

  def test_tautology
    assert true
  end

  def test_empty
    # Empty test method with zero assertions
  end
end

RSpec.describe "DisabledFeatures" do
  xit "is pending implementation" do
    expect(true).to eq(true)
  end
end
