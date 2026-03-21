#include <stdio.h>
#include <math.h>
#include "vec3.h"
#include "matrix.h"

#define PI 3.14159265358979323846f

int main(void) {
    printf("=== mathlib demo ===\n\n");

    /* vector basics */
    Vec3 a = vec3_new(1, 2, 3);
    Vec3 b = vec3_new(4, 5, 6);

    printf("a = "); vec3_print(a); printf("\n");
    printf("b = "); vec3_print(b); printf("\n");

    Vec3 sum = vec3_add(a, b);
    printf("a + b = "); vec3_print(sum); printf("\n");

    float dot = vec3_dot(a, b);
    printf("a . b = %.3f\n", dot);

    Vec3 cross = vec3_cross(a, b);
    printf("a x b = "); vec3_print(cross); printf("\n");

    Vec3 norm = vec3_normalize(a);
    printf("norm(a) = "); vec3_print(norm);
    printf("  (length = %.3f)\n", vec3_length(norm));

    /* matrix transforms */
    printf("\n--- transforms ---\n");

    Vec3 point = vec3_new(1, 0, 0);
    printf("point = "); vec3_print(point); printf("\n");

    Mat4 t = mat4_translate(vec3_new(10, 20, 30));
    Vec3 translated = mat4_transform_point(t, point);
    printf("translated = "); vec3_print(translated); printf("\n");

    Mat4 s = mat4_scale(vec3_new(2, 2, 2));
    Vec3 scaled = mat4_transform_point(s, point);
    printf("scaled = "); vec3_print(scaled); printf("\n");

    Mat4 r = mat4_rotate_y(PI / 2.0f);
    Vec3 rotated = mat4_transform_point(r, point);
    printf("rotated 90deg Y = "); vec3_print(rotated); printf("\n");

    /* chained: scale then translate */
    Mat4 combined = mat4_multiply(t, s);
    Vec3 combo = mat4_transform_point(combined, point);
    printf("scale(2) then translate(10,20,30) = ");
    vec3_print(combo); printf("\n");

    printf("\nrotation matrix (90 deg Y):\n");
    mat4_print(r);

    printf("\ndone.\n");
    return 0;
}
