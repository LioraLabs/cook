#include "mathlib/vec.h"
#include <cstdio>

#define ASSERT_FLOAT_EQ(a, b) do { \
    if (mathlib::abs((a) - (b)) > 1e-5f) { \
        std::fprintf(stderr, "FAIL: %s:%d: %s = %f, expected %f\n", \
                     __FILE__, __LINE__, #a, (double)(a), (double)(b)); \
        failures++; \
    } \
} while(0)

int main() {
    int failures = 0;

    // Test addition
    {
        mathlib::Vec2 a(1, 2);
        mathlib::Vec2 b(3, 4);
        auto c = a + b;
        ASSERT_FLOAT_EQ(c.x, 4.0f);
        ASSERT_FLOAT_EQ(c.y, 6.0f);
    }

    // Test subtraction
    {
        mathlib::Vec2 a(5, 3);
        mathlib::Vec2 b(2, 1);
        auto c = a - b;
        ASSERT_FLOAT_EQ(c.x, 3.0f);
        ASSERT_FLOAT_EQ(c.y, 2.0f);
    }

    // Test scalar multiply
    {
        mathlib::Vec2 a(2, 3);
        auto c = a * 3.0f;
        ASSERT_FLOAT_EQ(c.x, 6.0f);
        ASSERT_FLOAT_EQ(c.y, 9.0f);
    }

    // Test dot product
    {
        mathlib::Vec2 a(1, 0);
        mathlib::Vec2 b(0, 1);
        ASSERT_FLOAT_EQ(a.dot(b), 0.0f);
        ASSERT_FLOAT_EQ(a.dot(a), 1.0f);
    }

    // Test length
    {
        mathlib::Vec2 a(3, 4);
        ASSERT_FLOAT_EQ(a.length(), 5.0f);
    }

    // Test normalized
    {
        mathlib::Vec2 a(3, 4);
        auto n = a.normalized();
        ASSERT_FLOAT_EQ(n.length(), 1.0f);
        ASSERT_FLOAT_EQ(n.x, 0.6f);
        ASSERT_FLOAT_EQ(n.y, 0.8f);
    }

    if (failures == 0) {
        std::printf("All vec tests passed!\n");
        return 0;
    } else {
        std::printf("%d vec test(s) FAILED\n", failures);
        return 1;
    }
}
