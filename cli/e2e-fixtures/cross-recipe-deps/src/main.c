#include <stdio.h>
#include "math/math.h"
#include "str/str.h"

int main(void) {
    printf("add(2, 3) = %d\n", add(2, 3));
    printf("mul(4, 5) = %d\n", mul(4, 5));

    char buf[] = "hello";
    reverse(buf);
    printf("reverse(\"hello\") = %s\n", buf);

    upper(buf);
    printf("upper(\"%s\") = %s\n", "olleh", buf);

    return 0;
}
