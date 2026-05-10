#!/bin/sh
# Fake luarocks for golden argv tests. Records the argv it was invoked with
# to a file pointed to by $FAKE_LUAROCKS_LOG, then exits with $FAKE_LUAROCKS_EXIT
# (default 0). Stdout is whatever $FAKE_LUAROCKS_STDOUT contains.

if [ -n "$FAKE_LUAROCKS_LOG" ]; then
    echo "argv:" > "$FAKE_LUAROCKS_LOG"
    for a in "$@"; do
        printf '  %s\n' "$a" >> "$FAKE_LUAROCKS_LOG"
    done
fi

if [ -n "$FAKE_LUAROCKS_STDOUT" ]; then
    printf '%s\n' "$FAKE_LUAROCKS_STDOUT"
fi

if [ -n "$FAKE_LUAROCKS_STDERR" ]; then
    printf '%s\n' "$FAKE_LUAROCKS_STDERR" >&2
fi

exit "${FAKE_LUAROCKS_EXIT:-0}"
