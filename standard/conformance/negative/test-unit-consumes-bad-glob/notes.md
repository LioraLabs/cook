§22.4, §17.4 rule 1, CS-0175. An unparseable `consumes` pattern MUST be
rejected at register phase, naming the offending entry.

The reason this is a hard error rather than a tolerated no-op is the
direction of the failure. `consumes` is an allowlist over a dependency's
outputs, so a pattern that matches nothing removes that dependency's
content from the test's cache key entirely — and a test whose key has
lost a determinant replays a cached PASS against inputs that have moved.
Silent, green, and wrong. Every other narrowing surface in the Standard
fails toward over-invalidation; this one would fail toward a stale pass,
so the unparseable case is caught where the author can still see it.

The runtime counterpart of the same concern is normative in §17.4 rule
1: a syntactically valid filter that happens to match none of the
available predecessor outputs does not take effect either — the
unnarrowed set folds instead.
