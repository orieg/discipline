using System;
using Xunit;

namespace Discipline.Tests
{
    public class SkipsAndVacuousTest
    {
        [Fact(Skip = "Temporarily disabled in CI")]
        public void TestSkippedFact()
        {
            Assert.Equal(1, 1);
        }

        [Fact]
        public void TestTautology()
        {
            Assert.True(true);
        }

        [Fact]
        public void TestEmptyMethod()
        {
            // Empty method with zero assertions
        }
    }
}
