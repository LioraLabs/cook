#!/usr/bin/env bash
#
# Cook Standard normative-keyword lint.
#
# Flags lowercase occurrences of must/shall/should/may as whole words inside
# normative chapters of the Cook Standard. RFC 2119 keywords MUST appear in
# all-caps when they carry normative weight (§ 1.1). This lint catches
# accidental de-normative-ization during edits.
#
# Each flagged line is a candidate: the reviewer either promotes the keyword
# to all-caps (if the clause should be binding) or rewords (if the clause is
# descriptive).

set -euo pipefail

NORMATIVE_GLOB='src/content/docs/0[0-9]-*.mdx src/content/docs/appendix/A-*.mdx'

hits=0
for f in $NORMATIVE_GLOB; do
  [ -f "$f" ] || continue
  # -E extended regex; \b word boundaries. Skip lines inside fenced code blocks
  # by excluding lines that start with a triple backtick or sit between them.
  # We approximate: strip fenced regions with awk, then grep.
  filtered="$(awk '
    BEGIN { in_fence = 0 }
    /^```/ { in_fence = !in_fence; next }
    { if (!in_fence) print NR ":" $0 }
  ' "$f")"

  matches="$(printf '%s\n' "$filtered" | grep -E '\b(must|shall|should|may)\b' || true)"

  if [ -n "$matches" ]; then
    echo "== $f =="
    printf '%s\n' "$matches"
    hits=$((hits + 1))
  fi
done

if [ "$hits" -gt 0 ]; then
  echo ""
  echo "check-normative-keywords: lowercase RFC 2119 keywords found in $hits file(s)."
  echo "Review each hit: promote to all-caps if the clause is binding, or"
  echo "reword to remove the keyword if the clause is descriptive."
  exit 1
fi

echo "check-normative-keywords: OK"
exit 0
