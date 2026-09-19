#include <gtest/gtest.h>

TEST(CleanSuite, Addition) {
    int sum = 20 + 22;
    EXPECT_EQ(sum, 42);
    ASSERT_GT(sum, 0);
}

TEST(CleanSuite, StringMatch) {
    std::string text = "hello world";
    EXPECT_EQ(text, "hello world");
}
