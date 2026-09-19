#include <gtest/gtest.h>

TEST(SkipsAndVacuous, DISABLED_SkippedTest) {
    EXPECT_EQ(1, 1);
}

TEST(SkipsAndVacuous, Tautology) {
    EXPECT_TRUE(true);
}

TEST(SkipsAndVacuous, EmptyTest) {
    // Empty test with zero assertions
}
