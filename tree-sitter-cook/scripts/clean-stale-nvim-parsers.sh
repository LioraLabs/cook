#!/usr/bin/env bash
# Archive stale nvim-treesitter-distributed cook.so parsers.
#
# Stale plugin-distributed cook.so (nvim-treesitter via lazy.nvim,
# site/pack, etc.) sits earlier in &rtp than $NVIM_SITE and shadows the
# freshly-installed parser, so local edits never reach the editor. Move
# conflicting copies aside (timestamped .bak) — non-destructive, and
# Neovim stops loading them.

set -euo pipefail

: "${NVIM_SITE:=${XDG_DATA_HOME:-$HOME/.local/share}/nvim/site}"

nvim_data="$(dirname "$NVIM_SITE")"
nvim_config="${XDG_CONFIG_HOME:-$HOME/.config}/nvim"
stamp=$(date +%s)

find "$nvim_data" "$nvim_config" -maxdepth 6 -name cook.so \
    -path '*/parser/cook.so' \
    -not -path "$NVIM_SITE/parser/cook.so" 2>/dev/null \
| while read -r f; do
    mv "$f" "$f.bak.$stamp"
    echo "backed up stale parser → $f.bak.$stamp"
done
