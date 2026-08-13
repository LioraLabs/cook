#!/usr/bin/env bash
# Build every module-free example and fail if one does not.
#
# examples/ is what a stranger reads first, and the README tour points at it.
# It rotted anyway: five cook_cc showcases sat broken long enough that fixing
# them was no longer worth it against migrating them, and they were deleted
# (COOK-453). Nothing had ever built them, so two separate surface changes —
# the engine's requirement that target makers be called inside a `recipe`
# block, and cook_cc's own `config_header` signature — landed without anyone
# noticing what they invalidated.
#
# The entry point is stated per example rather than discovered. A bare `cook`
# resolves a recipe named `build`, and most of these deliberately do not have
# one: `02-pipeline` produces a `report`, `09-deploy` produces a `site`. Running
# the wrong target reports "recipe not found: build", which reads exactly like a
# broken example and is not one — a mistake worth spending four lines of table
# to make impossible.
#
# Module-dependent examples are NOT covered here. `examples/modules/monorepo`
# needs `cook modules install` (network) plus a pnpm toolchain, which is not a
# dependency this gate should acquire. It is excluded loudly rather than
# quietly skipped: see the closing note.
#
# Run from the repo root (the root Cookfile's member directory).
set -uo pipefail

cook=${COOK:-cli/target/release/cook}

if [ ! -x "$cook" ]; then
	echo "check-examples: no cook binary at $cook" >&2
	exit 1
fi
cook=$(cd "$(dirname "$cook")" && pwd)/$(basename "$cook")
root=$(pwd)

# example directory <TAB> target to build ("" means the default recipe)
examples=$(
	cat <<-'TABLE'
		01-hello-cook
		02-pipeline	report
		03-chores-and-config
		04-probes	targets
		05-data-fanout	summary
		06-lua-recipes	rot13
		07-testing	check
		08-workspace
		09-deploy	site
		10-cache-trust
		directory_hopping
	TABLE
)

status=0
ran=0

while IFS=$'\t' read -r dir target; do
	[ -n "${dir:-}" ] || continue
	path="$root/examples/$dir"

	if [ ! -d "$path" ]; then
		printf 'examples/%s: listed in this gate but not on disk\n' "$dir" >&2
		status=1
		continue
	fi

	ran=$((ran + 1))
	if ! out=$(cd "$path" && "$cook" ${target:+"$target"} 2>&1); then
		printf '\n== examples/%s%s ==\n%s\n' "$dir" "${target:+ ($target)}" "$out" >&2
		status=1
	fi
done <<<"$examples"

# An example added to the tree but not to the table is invisible to this gate,
# which is the failure this gate exists to prevent, one level up.
on_disk=$(cd "$root/examples" && find . -maxdepth 1 -mindepth 1 -type d ! -name modules ! -name '.*' -printf '%f\n' | sort)
listed=$(cut -f1 <<<"$examples" | grep -v '^$' | sort)
unlisted=$(comm -23 <(echo "$on_disk") <(echo "$listed"))

if [ -n "$unlisted" ]; then
	printf '\nexamples present but not covered by this gate:\n%s\n' "$unlisted" >&2
	printf 'add each to the table in scripts/check-examples.sh with the target it builds\n' >&2
	status=1
fi

if [ "$status" -ne 0 ]; then
	exit 1
fi

echo "$ran module-free examples built (examples/modules/monorepo excluded: needs network + pnpm)"
