#include "str.h"
#include <ctype.h>

void upper(char *s) {
    for (; *s; s++) {
        *s = toupper(*s);
    }
}
