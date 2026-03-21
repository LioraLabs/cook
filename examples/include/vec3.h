#ifndef VEC3_H
#define VEC3_H

typedef struct {
    float x, y, z;
} Vec3;

Vec3 vec3_new(float x, float y, float z);
Vec3 vec3_add(Vec3 a, Vec3 b);
Vec3 vec3_sub(Vec3 a, Vec3 b);
Vec3 vec3_scale(Vec3 v, float s);
float vec3_dot(Vec3 a, Vec3 b);
Vec3 vec3_cross(Vec3 a, Vec3 b);
float vec3_length(Vec3 v);
Vec3 vec3_normalize(Vec3 v);
void vec3_print(Vec3 v);

#endif
