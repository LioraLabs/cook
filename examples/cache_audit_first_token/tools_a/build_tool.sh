#!/bin/sh
# tools_a — variant A of the build tool.
in="$1"
out="$2"
printf 'TOOL_A: ' > "$out"
cat "$in" >> "$out"
