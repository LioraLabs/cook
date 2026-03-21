#include "matrix.h"
#include <math.h>
#include <stdio.h>
#include <string.h>

Mat4 mat4_identity(void) {
    Mat4 m;
    memset(&m, 0, sizeof(m));
    m.m[0] = m.m[5] = m.m[10] = m.m[15] = 1.0f;
    return m;
}

Mat4 mat4_multiply(Mat4 a, Mat4 b) {
    Mat4 result;
    memset(&result, 0, sizeof(result));
    for (int col = 0; col < 4; col++) {
        for (int row = 0; row < 4; row++) {
            float sum = 0.0f;
            for (int k = 0; k < 4; k++) {
                sum += a.m[k * 4 + row] * b.m[col * 4 + k];
            }
            result.m[col * 4 + row] = sum;
        }
    }
    return result;
}

Mat4 mat4_translate(Vec3 t) {
    Mat4 m = mat4_identity();
    m.m[12] = t.x;
    m.m[13] = t.y;
    m.m[14] = t.z;
    return m;
}

Mat4 mat4_scale(Vec3 s) {
    Mat4 m = mat4_identity();
    m.m[0]  = s.x;
    m.m[5]  = s.y;
    m.m[10] = s.z;
    return m;
}

Mat4 mat4_rotate_y(float radians) {
    Mat4 m = mat4_identity();
    float c = cosf(radians);
    float s = sinf(radians);
    m.m[0]  =  c;
    m.m[2]  =  s;
    m.m[8]  = -s;
    m.m[10] =  c;
    return m;
}

Vec3 mat4_transform_point(Mat4 m, Vec3 p) {
    float x = m.m[0]*p.x + m.m[4]*p.y + m.m[8]*p.z  + m.m[12];
    float y = m.m[1]*p.x + m.m[5]*p.y + m.m[9]*p.z  + m.m[13];
    float z = m.m[2]*p.x + m.m[6]*p.y + m.m[10]*p.z + m.m[14];
    return (Vec3){x, y, z};
}

void mat4_print(Mat4 m) {
    for (int row = 0; row < 4; row++) {
        printf("| %7.3f %7.3f %7.3f %7.3f |\n",
            m.m[row], m.m[4+row], m.m[8+row], m.m[12+row]);
    }
}
