#!/usr/bin/env bash
#
# Verify cook-lang against the Cook Standard corpus from a previously-cut
# version. Usage:
#
#     standard/scripts/check-conformance-against-tag.sh v0.1
#
# Materializes standard/conformance/ from the cs-standard/<version> tag into
# a temporary directory and runs the conformance harness with
# COOK_CONFORMANCE_CORPUS pointed at that directory.
#
# See standard/specs/2026-04-26-cli-standard-conformance-workflow-design.md.

set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <version>  (e.g. v0.1)" >&2
  exit 2
fi

version="$1"
tag="cs-standard/${version}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! git rev-parse --verify --quiet "$tag" >/dev/null; then
  echo "error: tag '$tag' not found in this repository" >&2
  exit 1
fi

tmpdir="$(mktemp -d -t cook-conformance-XXXXXX)"
trap 'rm -rf "$tmpdir"' EXIT

# Restore the conformance/ subtree from the tag into tmpdir/conformance.
git archive "$tag" "standard/conformance" \
  | tar -x -C "$tmpdir" --strip-components=1

if [ ! -d "$tmpdir/conformance/positive" ]; then
  echo "error: tag '$tag' did not contain standard/conformance/positive" >&2
  exit 1
fi

echo "Running cook-lang conformance harness against $tag"
echo "Corpus: $tmpdir/conformance"

COOK_CONFORMANCE_CORPUS="$tmpdir/conformance" \
  cargo test --manifest-path "$repo_root/cli/Cargo.toml" \
    -p cook-lang --test conformance
