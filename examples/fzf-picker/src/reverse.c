#include <stdio.h>
#include <string.h>

int main(void) {
    char buf[4096];
    while (fgets(buf, sizeof buf, stdin)) {
        size_t n = strlen(buf);
        if (n > 0 && buf[n - 1] == '\n') {
            buf[--n] = '\0';
        }
        for (size_t i = n; i-- > 0;) {
            putchar(buf[i]);
        }
        putchar('\n');
    }
    return 0;
}
