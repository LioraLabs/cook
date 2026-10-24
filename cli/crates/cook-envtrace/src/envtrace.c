/* Records the NAME of every environment variable the process queries through
 * getenv/secure_getenv, one write() per name to the file named by OUT_VAR
 * (O_APPEND, so atomic across processes). Never records values.
 * A line is `pid tid index long count NAME`: the name's position in `environ`
 * (`-` when unset), whether its value is 256 bytes or more (`1`/`0`; the value
 * and its length are never recorded) and environ's length, so the reader can
 * tell a walk of the whole environment (node copying process.env) from a point
 * lookup.
 * The file is opened per record, not held in a descriptor: a held one sits at
 * the lowest free number, which the program may later dup2 over or close and
 * reuse, and names would then land in an unrelated file. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

extern char **environ;
typedef char *(*getenv_fn)(const char *);

static getenv_fn real_getenv, real_secure;
static char out_path[4096];

/* The variable by scanning `environ` ourselves: used when the real function
 * could not be resolved, so a set variable never looks unset. */
static char *scan_environ(const char *name) {
    size_t len = strlen(name);
    char **e;
    if (!environ) return 0;
    for (e = environ; *e; e++)
        if (strncmp(*e, name, len) == 0 && (*e)[len] == '=') return *e + len + 1;
    return 0;
}

/* Appends `n` in decimal and a space; no printf, which could recurse into getenv. */
static size_t put_num(char *p, unsigned long n) {
    char tmp[24];
    size_t k = 0, i;
    do tmp[k++] = (char)('0' + n % 10); while (n /= 10);
    for (i = 0; i < k; i++) p[i] = tmp[k - 1 - i];
    p[k] = ' ';
    return k + 1;
}

static void record(const char *name) {
    char buf[600];
    size_t len, at = 0, n = 0, idx = 0;
    int found = 0, fd, is_long = 0;
    char **e;
    int saved = errno; /* an interposer must not change the caller's errno */
    if (!out_path[0] || !name) return;
    len = strlen(name);
    if (environ)
        for (e = environ; *e; e++, n++)
            if (!found && strncmp(*e, name, len) == 0 && (*e)[len] == '=') {
                found = 1, idx = n;
                /* node/libuv's 256-byte buffer makes it look a long value up again. */
                is_long = strlen(*e + len + 1) >= 256;
            }
    at += put_num(buf + at, (unsigned long)getpid());
    at += put_num(buf + at, (unsigned long)syscall(SYS_gettid));
    if (found) at += put_num(buf + at, idx);
    else buf[at++] = '-', buf[at++] = ' ';
    at += put_num(buf + at, is_long);
    at += put_num(buf + at, n);
    /* Names this long do not occur; the first bytes still say which it was. */
    if (len > sizeof buf - 1 - at) len = sizeof buf - 1 - at;
    memcpy(buf + at, name, len);
    at += len;
    buf[at++] = '\n';
    fd = open(out_path, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0600);
    if (fd >= 0) {
        (void)!write(fd, buf, at);
        close(fd);
    }
    errno = saved;
}

__attribute__((constructor)) static void init(void) {
    const char *path;
    real_getenv = (getenv_fn)dlsym(RTLD_NEXT, "getenv");
    real_secure = (getenv_fn)dlsym(RTLD_NEXT, "secure_getenv");
    path = real_getenv ? real_getenv(OUT_VAR) : scan_environ(OUT_VAR);
    if (path && strlen(path) < sizeof out_path) strcpy(out_path, path);
}

char *getenv(const char *name) {
    record(name);
    if (real_getenv) return real_getenv(name);
    return name ? scan_environ(name) : 0;
}

char *secure_getenv(const char *name) {
    record(name);
    if (real_secure) return real_secure(name);
    /* This libc has no secure_getenv: a plain lookup, never a false "unset". */
    return name ? scan_environ(name) : 0;
}
