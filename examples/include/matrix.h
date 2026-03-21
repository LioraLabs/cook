#ifndef MATRIX_H
#define MATRIX_H

#include "vec3.h"

/* 4x4 matrix in column-major order */
typedef struct {
    float m[16];
} Mat4;

Mat4 mat4_identity(void);
Mat4 mat4_multiply(Mat4 a, Mat4 b);
Mat4 mat4_translate(Vec3 t);
Mat4 mat4_scale(Vec3 s);
Mat4 mat4_rotate_y(float radians);
Vec3 mat4_transform_point(Mat4 m, Vec3 p);
void mat4_print(Mat4 m);

#endif
