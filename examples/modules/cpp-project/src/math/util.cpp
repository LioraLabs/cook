#include "mathlib/util.h"

// util.h is header-only, but we include this translation unit
// to test that the build system handles files with no unique symbols.
// This also validates that the include path resolution works.

namespace mathlib {
namespace detail {
    // Placeholder to ensure this TU is not empty
    static const int util_version = 1;
}
}
