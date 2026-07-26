#!/usr/bin/env bash
# Check cook-lang against the conformance corpus as it stood at a published
# Standard tag, rather than as it stands in the working tree.
#
# Imperative by nature: it reconstructs a tree from git history, runs a harness
# against it, and tears it down. That is a chore, not a work unit — there is no
# artifact to cache and no determinant to seal, because the whole point is to
# read a corpus that is not on disk. What it is not is a reason to assemble the
# pipeline as a Lua string; this is the same commands, in a file a shell can run.
#
# Run from standard/ (the member root). Argument: a Standard version, with or
# without the leading `v` — `0.18` and `v0.18` both work.
set -euo pipefail

version=${1:?usage: against-tag.sh VERSION (e.g. 0.18)}
version=v${version#v}
tag="cs-standard/${version}"

if ! git rev-parse --verify --quiet "$tag" >/dev/null; then
	echo "against-tag: tag '$tag' not found in this repository" >&2
	exit 1
fi

# `git -C "$repo_root"` so the pathspec resolves repo-relative even though this
# runs with cwd = standard/. The corpus path handed to the harness is absolute
# because `cargo test` runs the test binary with cwd = crate root, so a relative
# one would no longer resolve there.
repo_root=$(git rev-parse --show-toplevel)
tmpdir="${repo_root}/standard/.cook/conformance-${version}"
corpus="${tmpdir}/conformance"

cleanup() { rm -rf "$tmpdir"; }
trap cleanup EXIT

rm -rf "$tmpdir"
mkdir -p "$tmpdir"
git -C "$repo_root" archive "$tag" standard/conformance |
	tar -x -C "$tmpdir" --strip-components=1

if [ ! -d "${corpus}/positive" ]; then
	echo "against-tag: $tag did not contain standard/conformance/positive" >&2
	exit 1
fi

echo "Running cook-lang conformance harness against ${tag}"
echo "Corpus: ${corpus}"
COOK_CONFORMANCE_CORPUS="$corpus" \
	cargo test --manifest-path "${repo_root}/cli/Cargo.toml" \
	-p cook-lang --test conformance
