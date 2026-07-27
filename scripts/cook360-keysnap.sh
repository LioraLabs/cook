#!/bin/bash
# Snapshot cache keys + determinants across every e2e fixture, driving each
# recipe the fixture actually declares. mtimes are stripped: they are
# observations of the filesystem, not part of any key.
W="$1"; OUT="$2"
WORK=$(mktemp -d); : > "$OUT"
for d in "$W"/cli/e2e-fixtures/*/; do
  f=$(basename "$d")
  [ -f "$d/Cookfile" ] || continue
  cp -r "$d" "$WORK/$f" 2>/dev/null || continue
  cd "$WORK/$f" || continue
  recipes=$(grep -oE '^recipe +[A-Za-z0-9_.-]+' Cookfile | awk '{print $2}' | sort -u)
  for r in $recipes; do
    timeout 30 "$W/cli/target/debug/cook" "$r" >/dev/null 2>&1
    dump=$(timeout 15 "$W/cli/target/debug/cook" cache dump "$r" 2>/dev/null | sed -E 's/mtime = [0-9]+/mtime = _/g')
    [ -n "$dump" ] && { echo "##### $f :: $r"; echo "$dump"; } >> "$OUT"
  done
done
rm -rf "$WORK"
# Keys-only view. The full dump also contains OUTPUT CONTENT hashes, which are
# not a function of the code under test: they are restored from the shared
# store at ~/.cache/cook when its namespace is intact, and rebuilt (so, for a
# -g build in a fresh temp dir, byte-different) when a version bump
# invalidates it. Compare THIS file to judge whether a change moved a key.
grep -E '^\[steps\.|^command_hash|^env_contribution|^seal_contribution|^#####' "$OUT" > "$OUT.keys"
