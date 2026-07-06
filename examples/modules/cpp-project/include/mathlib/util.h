#ifndef MATHLIB_UTIL_H
#define MATHLIB_UTIL_H

namespace mathlib {

inline float clamp(float val, float lo, float hi) {
    if (val < lo) return lo;
    if (val > hi) return hi;
    return val;
}

inline float abs(float val) {
    return val < 0 ? -val : val;
}

}  // namespace mathlib

#endif  // MATHLIB_UTIL_H
