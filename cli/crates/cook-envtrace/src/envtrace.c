/* Records the NAME of every environment variable the process queries through
 * getenv/secure_getenv, one write() per name to the file named by OUT_VAR
 * (O_APPEND, so atomic across processes). Never records values.
 * The file is opened per record, not held in a descriptor: a held one sits at
 * the lowest free number, which the program may later dup2 over or close and
 * reuse, and names would then land in an unrelated file. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
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

static void record(const char *name) {
    char buf[512];
    size_t len;
    int fd;
    int saved = errno; /* an interposer must not change the caller's errno */
    if (!out_path[0] || !name) return;
    len = strlen(name);
    /* Names this long do not occur; the first bytes still say which it was. */
    if (len > sizeof buf - 1) len = sizeof buf - 1;
    memcpy(buf, name, len);
    buf[len] = '\n';
    fd = open(out_path, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0600);
    if (fd >= 0) {
        (void)!write(fd, buf, len + 1);
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
