#include "mathlib/vec.h"
#include <cstdio>

int main() {
    mathlib::Vec2 a(3.0f, 4.0f);
    mathlib::Vec2 b(1.0f, 2.0f);

    auto sum = a + b;
    auto diff = a - b;
    auto scaled = a * 2.0f;
    float d = a.dot(b);
    auto n = a.normalized();

    std::printf("a + b = (%.1f, %.1f)\n", sum.x, sum.y);
    std::printf("a - b = (%.1f, %.1f)\n", diff.x, diff.y);
    std::printf("a * 2 = (%.1f, %.1f)\n", scaled.x, scaled.y);
    std::printf("a . b = %.1f\n", d);
    std::printf("|a| = %.2f\n", a.length());
    std::printf("norm(a) = (%.2f, %.2f)\n", n.x, n.y);

    return 0;
}
