/*
 * Cook M3 demo — proves cc.config_header + cc.checks.* work end-to-end.
 *
 * The included config.h is generated at build time from
 * raylib-src/src/config.h.in by cook_cc.config_header(...), using
 * cc.checks.has_header / has_function results plus literal scalar
 * vars declared in the Cookfile.
 *
 * Vendored upstream sources are from raysan5/raylib 5.5 (zlib/libpng).
 * A full raylib library build is intentionally out of scope here — see
 * README.md for the rationale.
 */
#include <stdio.h>

#include "config.h"
#include "raylib.h"

int main(void)
{
    puts("Cook M3 example -- generated config.h\n");

    printf("  RAYLIB_VERSION = %s\n", RAYLIB_VERSION);
    printf("  HAVE_STDINT_H  = %d\n", HAVE_STDINT_H);
    printf("  HAVE_STRDUP    = %d\n", HAVE_STRDUP);

#ifdef SUPPORT_MODULE_RCORE
    puts("  SUPPORT_MODULE_RCORE     = ON");
#else
    puts("  SUPPORT_MODULE_RCORE     = OFF");
#endif

#ifdef SUPPORT_MODULE_RSHAPES
    puts("  SUPPORT_MODULE_RSHAPES   = ON");
#else
    puts("  SUPPORT_MODULE_RSHAPES   = OFF");
#endif

#ifdef SUPPORT_MODULE_RTEXTURES
    puts("  SUPPORT_MODULE_RTEXTURES = ON");
#else
    puts("  SUPPORT_MODULE_RTEXTURES = OFF");
#endif

#ifdef SUPPORT_QUADS_DRAW_MODE
    puts("  SUPPORT_QUADS_DRAW_MODE  = ON");
#else
    puts("  SUPPORT_QUADS_DRAW_MODE  = OFF");
#endif

#ifdef SUPPORT_FILEFORMAT_PNG
    puts("  SUPPORT_FILEFORMAT_PNG   = ON");
#else
    puts("  SUPPORT_FILEFORMAT_PNG   = OFF");
#endif

    return 0;
}
