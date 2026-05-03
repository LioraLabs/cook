#!/bin/sh
# tools_b — variant B of the build tool.
# Different bytes ⇒ different SHA-256 ⇒ post-fix the context hash differs
# and the cache invalidates.
in="$1"
out="$2"
printf 'TOOL_B (different bytes here to perturb sha256): ' > "$out"
cat "$in" >> "$out"
