#include <stdio.h>
#include <math.h>
#include "vec3.h"
#include "matrix.h"

#define PI 3.14159265358979323846f

#define ASSERT_NEAR(a, b, eps, msg) do { \
    if (fabsf((a) - (b)) > (eps)) { \
        printf("FAIL: %s (expected %.6f, got %.6f)\n", msg, (float)(b), (float)(a)); \
        failures++; \
    } \
} while(0)

int main(void) {
    int failures = 0;

    /* identity * point = point */
    Mat4 id = mat4_identity();
    Vec3 p = vec3_new(3, 4, 5);
    Vec3 r = mat4_transform_point(id, p);
    ASSERT_NEAR(r.x, 3.0f, 1e-5f, "identity.x");
    ASSERT_NEAR(r.y, 4.0f, 1e-5f, "identity.y");
    ASSERT_NEAR(r.z, 5.0f, 1e-5f, "identity.z");

    /* translate */
    Mat4 t = mat4_translate(vec3_new(10, 20, 30));
    r = mat4_transform_point(t, vec3_new(1, 1, 1));
    ASSERT_NEAR(r.x, 11.0f, 1e-5f, "translate.x");
    ASSERT_NEAR(r.y, 21.0f, 1e-5f, "translate.y");
    ASSERT_NEAR(r.z, 31.0f, 1e-5f, "translate.z");

    /* scale */
    Mat4 s = mat4_scale(vec3_new(2, 3, 4));
    r = mat4_transform_point(s, vec3_new(1, 1, 1));
    ASSERT_NEAR(r.x, 2.0f, 1e-5f, "scale.x");
    ASSERT_NEAR(r.y, 3.0f, 1e-5f, "scale.y");
    ASSERT_NEAR(r.z, 4.0f, 1e-5f, "scale.z");

    /* rotate Y 90 degrees: (1,0,0) -> (0,0,1) in our convention */
    Mat4 rot = mat4_rotate_y(PI / 2.0f);
    r = mat4_transform_point(rot, vec3_new(1, 0, 0));
    ASSERT_NEAR(r.x, 0.0f, 1e-4f, "rot90.x");
    ASSERT_NEAR(r.y, 0.0f, 1e-4f, "rot90.y");
    ASSERT_NEAR(fabsf(r.z), 1.0f, 1e-4f, "rot90.z magnitude");

    /* multiply: scale then translate */
    Mat4 combo = mat4_multiply(t, s);
    r = mat4_transform_point(combo, vec3_new(1, 1, 1));
    ASSERT_NEAR(r.x, 12.0f, 1e-5f, "combo.x");  /* 1*2 + 10 */
    ASSERT_NEAR(r.y, 23.0f, 1e-5f, "combo.y");  /* 1*3 + 20 */
    ASSERT_NEAR(r.z, 34.0f, 1e-5f, "combo.z");  /* 1*4 + 30 */

    if (failures == 0) {
        printf("matrix: all tests passed\n");
    } else {
        printf("matrix: %d test(s) failed\n", failures);
    }
    return failures;
}
