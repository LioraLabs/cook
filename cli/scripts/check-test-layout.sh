#!/bin/sh
# Enforces the Rust test-layout convention (COOK-312). See CONTRIBUTING.md.
#
#   1. Unit tests live in <dir>/tests/<name>_tests.rs, attached with
#      `#[cfg(test)] #[path = ...] mod <M>;`. An inline `#[cfg(test)] mod M { }`
#      body in a source file is a violation: it puts test code back into the
#      files you navigate, which is the whole point of the convention.
#   2. Integration tests under a crate-root tests/ use bare descriptive names.
#      The directory already says "test", so the affix is noise.
#
# Run from cli/. Prints `path:line: reason` per violation; exits 1 if any.
set -u
cd "$(dirname "$0")/.." || exit 2
status=0

# Rule 1 --- inline #[cfg(test)] mod bodies in production source.
# Skips anything already under a tests/ directory. Tolerates attributes
# between #[cfg(test)] and the mod line (e.g. #[allow(non_snake_case)]).
for f in $(find crates/*/src -name '*.rs' -not -path '*/tests/*' | sort); do
    awk -v F="$f" '
        /^#\[cfg\(test\)\]/ { pending = 1; at = NR; next }
        pending && /^#\[/    { next }
        pending {
            if ($0 ~ /^mod [A-Za-z_][A-Za-z0-9_]* \{/)
                printf "%s:%d: inline #[cfg(test)] mod body; move it to %s/tests/ and attach with #[path]\n", \
                       F, at, substr(F, 1, match(F, /\/[^\/]*$/) - 1)
            pending = 0
        }
    ' "$f" && continue
done > /tmp/.check-test-layout.$$ 2>/dev/null
if [ -s /tmp/.check-test-layout.$$ ]; then
    cat /tmp/.check-test-layout.$$
    status=1
fi
rm -f /tmp/.check-test-layout.$$

# Rule 2 --- dead naming affixes on integration tests.
for f in $(find crates/*/tests -maxdepth 1 -name '*.rs' 2>/dev/null | sort); do
    b=$(basename "$f")
    case "$b" in
        *_e2e.rs|*_test.rs|*_integration.rs|test_*.rs|integration_*.rs)
            echo "$f:1: integration test name carries a dead affix; use a bare descriptive name"
            status=1
            ;;
    esac
done

[ "$status" -eq 0 ] && echo "test-layout: clean"
exit "$status"
