#!/usr/bin/env bash
# Install tree-sitter-cook queries for Neovim.
#
# Writes to $NVIM_SITE/queries/cook/ and detects stale copies at other
# Neovim runtimepath locations that Neovim would merge against the
# fresh install. Conflicting copies are archived to a timestamped
# `.bak.<epoch>` directory so Neovim stops loading them; nothing is
# deleted outright.

set -euo pipefail

: "${NVIM_SITE:=${XDG_DATA_HOME:-$HOME/.local/share}/nvim/site}"

src_dir="queries"
dst_dir="$NVIM_SITE/queries/cook"

mkdir -p "$dst_dir"
cp "$src_dir"/*.scm "$dst_dir/"
echo "installed queries → $dst_dir"

# Other rtp locations Neovim merges queries from. We deliberately do
# NOT touch plugin-managed dirs (lazy/, site/pack/) — those use a
# different parser name (cooklang) or are owned by a plugin manager.
candidates=(
    "${XDG_CONFIG_HOME:-$HOME/.config}/nvim/queries/cook"
)

stamp=$(date +%s)
for dir in "${candidates[@]}"; do
    [ -d "$dir" ] || continue
    [ "$(realpath "$dir")" = "$(realpath "$dst_dir")" ] && continue

    diffs=""
    for f in "$dir"/*.scm; do
        [ -e "$f" ] || continue
        name=$(basename "$f")
        fresh="$dst_dir/$name"
        if [ ! -e "$fresh" ] || ! cmp -s "$f" "$fresh"; then
            diffs="${diffs:+$diffs }$name"
        fi
    done

    if [ -n "$diffs" ]; then
        bak="${dir}.bak.${stamp}"
        mv "$dir" "$bak"
        echo "archived stale queries: $dir → $bak (differs: $diffs)"
    fi
done
