require 'minitest/autorun'

class CleanCalculatorTest < Minitest::Test
  def test_addition
    sum = 20 + 22
    assert_equal 42, sum
    assert sum > 0
  end

  def test_multiplication
    product = 6 * 7
    assert_equal 42, product
  end
end

RSpec.describe "StringFormatter" do
  it "formats greeting correctly" do
    greeting = "hello world"
    expect(greeting).to eq("hello world")
  end
end
