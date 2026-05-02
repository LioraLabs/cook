#!/bin/sh
# Helper for the `pair` multi-output recipe. Writes two files.
# Usage: genpair.sh OUT_FOO OUT_BAR
set -eu
out_foo="$1"
out_bar="$2"
mkdir -p "$(dirname "$out_foo")" "$(dirname "$out_bar")"
echo "foo content $(date -u +%Y-%m-%d)" > "$out_foo"
echo "bar content (paired with foo)" > "$out_bar"
