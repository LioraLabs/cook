#include <stdio.h>
#include <math.h>
#include "vec3.h"

#define ASSERT_NEAR(a, b, eps, msg) do { \
    if (fabsf((a) - (b)) > (eps)) { \
        printf("FAIL: %s (expected %.6f, got %.6f)\n", msg, (float)(b), (float)(a)); \
        failures++; \
    } \
} while(0)

int main(void) {
    int failures = 0;

    /* add */
    Vec3 sum = vec3_add(vec3_new(1,2,3), vec3_new(4,5,6));
    ASSERT_NEAR(sum.x, 5.0f, 1e-5f, "add.x");
    ASSERT_NEAR(sum.y, 7.0f, 1e-5f, "add.y");
    ASSERT_NEAR(sum.z, 9.0f, 1e-5f, "add.z");

    /* sub */
    Vec3 diff = vec3_sub(vec3_new(5,5,5), vec3_new(1,2,3));
    ASSERT_NEAR(diff.x, 4.0f, 1e-5f, "sub.x");
    ASSERT_NEAR(diff.y, 3.0f, 1e-5f, "sub.y");
    ASSERT_NEAR(diff.z, 2.0f, 1e-5f, "sub.z");

    /* dot */
    float d = vec3_dot(vec3_new(1,0,0), vec3_new(0,1,0));
    ASSERT_NEAR(d, 0.0f, 1e-5f, "orthogonal dot");

    d = vec3_dot(vec3_new(2,3,4), vec3_new(2,3,4));
    ASSERT_NEAR(d, 29.0f, 1e-5f, "self dot");

    /* cross */
    Vec3 c = vec3_cross(vec3_new(1,0,0), vec3_new(0,1,0));
    ASSERT_NEAR(c.x, 0.0f, 1e-5f, "cross.x");
    ASSERT_NEAR(c.y, 0.0f, 1e-5f, "cross.y");
    ASSERT_NEAR(c.z, 1.0f, 1e-5f, "cross.z");

    /* normalize */
    Vec3 n = vec3_normalize(vec3_new(3, 0, 0));
    ASSERT_NEAR(n.x, 1.0f, 1e-5f, "norm.x");
    ASSERT_NEAR(vec3_length(n), 1.0f, 1e-5f, "norm length");

    /* zero normalize */
    Vec3 z = vec3_normalize(vec3_new(0, 0, 0));
    ASSERT_NEAR(z.x, 0.0f, 1e-5f, "zero norm.x");

    if (failures == 0) {
        printf("vec3: all tests passed\n");
    } else {
        printf("vec3: %d test(s) failed\n", failures);
    }
    return failures;
}
