/* Consumer of lib_obj. The dep-edge is inferred from $<lib_obj> in
   the `app` recipe body — there's no #include relationship and main.c
   never changes during this test, so the only thing that can invalidate
   `app`'s cache is drift in lib.o's content arriving via the per-spec
   §4.3 cache_meta.input_paths recording. */
#include <stdio.h>
extern int lib_value(void);
int main(void) {
    printf("lib_value=%d\n", lib_value());
    return 0;
}
