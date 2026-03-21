#include "vec3.h"
#include <math.h>
#include <stdio.h>

Vec3 vec3_new(float x, float y, float z) {
    return (Vec3){x, y, z};
}

Vec3 vec3_add(Vec3 a, Vec3 b) {
    return (Vec3){a.x + b.x, a.y + b.y, a.z + b.z};
}

Vec3 vec3_sub(Vec3 a, Vec3 b) {
    return (Vec3){a.x - b.x, a.y - b.y, a.z - b.z};
}

Vec3 vec3_scale(Vec3 v, float s) {
    return (Vec3){v.x * s, v.y * s, v.z * s};
}

float vec3_dot(Vec3 a, Vec3 b) {
    return a.x * b.x + a.y * b.y + a.z * b.z;
}

Vec3 vec3_cross(Vec3 a, Vec3 b) {
    return (Vec3){
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x
    };
}

float vec3_length(Vec3 v) {
    return sqrtf(vec3_dot(v, v));
}

Vec3 vec3_normalize(Vec3 v) {
    float len = vec3_length(v);
    if (len < 1e-8f) return (Vec3){0, 0, 0};
    return vec3_scale(v, 1.0f / len);
}

void vec3_print(Vec3 v) {
    printf("(%6.3f, %6.3f, %6.3f)", v.x, v.y, v.z);
}
