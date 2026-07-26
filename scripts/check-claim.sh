#!/usr/bin/env bash
# Assert that every conformance-claim site agrees with standard/VERSION.
#
# The tag `cook release` pushes is what a downstream implementor reads the
# claim off, so the sites have to agree at exactly that moment. They once
# drifted three cuts apart (v0.15 vs v0.18) because nothing checked, and
# grammar.js in particular went unrewritten for several cuts while
# `release.bump_claim` silently matched no pattern. Each site is therefore
# checked explicitly rather than assumed to follow the Rust constant.
#
# Run from the repo root (the root Cookfile's member directory).
set -euo pipefail

declared=$(tr -d '[:space:]' < standard/VERSION)
status=0

check() {
	local label=$1 path=$2 found=$3
	if [ "$found" != "$declared" ]; then
		printf '%s claims v%s, standard/VERSION says v%s\n  %s\n' \
			"$label" "${found:-<no match>}" "$declared" "$path" >&2
		status=1
	fi
}

check "cook-lang" "cli/crates/cook-lang/src/lib.rs" \
	"$(sed -n 's/.*pub const COOK_STANDARD_VERSION: &str = "\([^"]*\)".*/\1/p' \
		cli/crates/cook-lang/src/lib.rs)"

check "tree-sitter-cook grammar" "tree-sitter-cook/grammar.js" \
	"$(sed -n 's/.*Conforms to Cook Standard v\([0-9]\+\.[0-9]\+\).*/\1/p' \
		tree-sitter-cook/grammar.js)"

for f in cli/crates/cook-lang/README.md cli/crates/cook-lang/CONFORMANCE.md; do
	check "$(basename "$f")" "$f" \
		"$(sed -n 's/.*claims \*\*Cook Standard v\([0-9.]\+\)\*\*.*/\1/p' "$f")"
done

for f in tree-sitter-cook/package.json tree-sitter-cook/tree-sitter.json; do
	check "$(basename "$f")" "$f" \
		"$(sed -n 's/.*conforming to Cook Standard v\([0-9.]\+\).*/\1/p' "$f")"
done

if [ "$status" -ne 0 ]; then
	echo >&2
	echo "run 'cook bump-claim' and commit the result" >&2
	exit 1
fi

echo "conformance claim in sync at v$declared"
