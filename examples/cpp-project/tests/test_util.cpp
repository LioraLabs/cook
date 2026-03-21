#include "mathlib/util.h"
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

    // Test clamp upper bound
    ASSERT_FLOAT_EQ(mathlib::clamp(5.0f, 0.0f, 1.0f), 1.0f);

    // Test clamp lower bound
    ASSERT_FLOAT_EQ(mathlib::clamp(-1.0f, 0.0f, 1.0f), 0.0f);

    // Test clamp in range
    ASSERT_FLOAT_EQ(mathlib::clamp(0.5f, 0.0f, 1.0f), 0.5f);

    // Test abs positive
    ASSERT_FLOAT_EQ(mathlib::abs(3.14f), 3.14f);

    // Test abs negative
    ASSERT_FLOAT_EQ(mathlib::abs(-2.5f), 2.5f);

    // Test abs zero
    ASSERT_FLOAT_EQ(mathlib::abs(0.0f), 0.0f);

    if (failures == 0) {
        std::printf("All util tests passed!\n");
        return 0;
    } else {
        std::printf("%d util test(s) FAILED\n", failures);
        return 1;
    }
}
