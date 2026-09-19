using System;
using Xunit;

namespace Discipline.Tests
{
    public class CleanSuiteTest
    {
        [Fact]
        public void TestAddition()
        {
            int sum = 20 + 22;
            Assert.Equal(42, sum);
            Assert.True(sum > 0);
        }

        [Theory]
        [InlineData(1)]
        [InlineData(2)]
        public void TestTheory(int val)
        {
            Assert.NotNull(val);
            Assert.Equal(val, val);
        }
    }
}
