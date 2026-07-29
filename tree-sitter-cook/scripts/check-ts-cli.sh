#!/usr/bin/env bash
# Assert that the tree-sitter CLI on PATH is exactly the one this repo pins.
#
# src/ is generated but tracked, so the CLI that generates it is source. It is
# also not byte-stable across versions: 0.25.10, 0.26.9 and 0.26.11 each emit
# different bytes for the same grammar.js (bd4fe6b0), and src/tree_sitter/*.h
# tracks the CLI rather than the grammar at all.
#
# `probe generate:tool` already OBSERVES which CLI ran — it is a determinant, so
# the key moves and `generate` correctly re-runs. What a determinant cannot do
# is make the *right* CLI the one that gets found. So the build did exactly what
# it was told: it re-ran `tree-sitter generate` under whatever PATH resolved and
# rewrote tracked source in place with the wrong CLI's bytes. `check-generated`
# then correctly reported drift it had not caused, and the dirty tree outlived
# the run and surfaced later as an unexplained `cook release` failure (COOK-356
# fixed that hazard inside check-generated.sh, but left it standing in the one
# recipe whose whole job is to write).
#
# This script is the missing constraint. It is wired as a `test` recipe that
# `generate` and every other CLI-invoking recipe depend on, so a failed check
# BLOCKS them (§8.6, CS-0177) rather than advising against them: the wrong CLI
# now fails before anything is written, leaving the tree untouched.
#
# Run from tree-sitter-cook/ (the member root).
set -euo pipefail

# package.json is the single source of truth for the pin. check-generated.sh
# reads the same field, and CI installs from the lockfile that resolves it, so
# there is exactly one place to edit when the CLI moves.
spec=$(sed -n 's/.*"tree-sitter-cli": *"\([^"]*\)".*/\1/p' package.json)

if [ -z "$spec" ]; then
	echo "check-ts-cli: no tree-sitter-cli entry in package.json" >&2
	echo "  the CLI version is the pin; it cannot be left unstated" >&2
	exit 1
fi

# A range is not a pin. `^0.26.11` admits 0.26.12, which emits different bytes
# for the same grammar and would land them in tracked source. Rejecting the
# range here is what keeps the caret from silently reappearing in a dependency
# bump, rather than trusting a comment to hold the line.
case "$spec" in
*[!0-9.]*)
	echo "check-ts-cli: package.json pins tree-sitter-cli as \"$spec\"" >&2
	echo >&2
	echo "  That is a RANGE, not a pin. Generated output is not stable across" >&2
	echo "  CLI versions, and src/ is tracked, so a range lets a routine bump" >&2
	echo "  rewrite committed parser bytes. Pin the exact version:" >&2
	echo >&2
	echo "    \"tree-sitter-cli\": \"${spec#[^0-9]}\"" >&2
	exit 1
	;;
esac

if ! command -v tree-sitter >/dev/null 2>&1; then
	echo "check-ts-cli: no tree-sitter on PATH; this repo pins $spec" >&2
	echo >&2
	echo "  Install the pinned CLI and put it on PATH:" >&2
	echo >&2
	echo "    (cd tree-sitter-cook && pnpm install)" >&2
	echo "    PATH=\"\$PWD/tree-sitter-cook/node_modules/.bin:\$PATH\" cook test" >&2
	exit 1
fi

actual=$(tree-sitter --version | sed -n 's/^tree-sitter \([0-9.]*\).*/\1/p')

if [ "$actual" != "$spec" ]; then
	echo "check-ts-cli: your tree-sitter is $actual; this repo pins $spec." >&2
	echo >&2
	echo "  Generated src/ is tracked and is NOT byte-stable across CLI" >&2
	echo "  versions, so running the generator under $actual would rewrite" >&2
	echo "  committed parser bytes. Nothing has been written." >&2
	echo >&2
	echo "  The pinned CLI is a devDependency. Install it and prefer it:" >&2
	echo >&2
	echo "    (cd tree-sitter-cook && pnpm install)" >&2
	echo "    PATH=\"\$PWD/tree-sitter-cook/node_modules/.bin:\$PATH\" cook test" >&2
	echo >&2
	echo "  A global install works too, but drifts the moment the pin moves:" >&2
	echo >&2
	echo "    npm install -g tree-sitter-cli@$spec" >&2
	exit 1
fi

echo "check-ts-cli: tree-sitter $actual matches the pin"
