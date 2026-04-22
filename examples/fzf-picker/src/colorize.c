#include <stdio.h>

int main(void) {
    const char *colors[] = {
        "\033[31m", "\033[33m", "\033[32m",
        "\033[36m", "\033[34m", "\033[35m",
    };
    char buf[4096];
    int i = 0;
    while (fgets(buf, sizeof buf, stdin)) {
        printf("%s%s\033[0m", colors[i++ % 6], buf);
    }
    return 0;
}
