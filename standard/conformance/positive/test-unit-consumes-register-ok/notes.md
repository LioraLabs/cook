§22.4, §17.4 rule 1, CS-0175. Pins register-phase success for the
`consumes` field on `cook.add_test`: a glob allowlist narrowing WHICH
immediate-predecessor outputs fold into the test's cache key. Empty (the
default) folds all of them, which is every prior revision's behaviour,
so this field is additive and no existing fixture changes key.

Both accepted pattern shapes appear, because the matching convention is
the part an implementation is most likely to get wrong: `*.d.ts` carries
no `/` and therefore matches a candidate's BASENAME at any depth, while
`dist/**/*.mjs` carries one and matches the workspace-root-relative
path. This is gitignore convention, chosen because it is the one every
author already has loaded.

The fixture registers a Lua-body test through a `register` block — the
same hand-authored module-call path a blessed module's target maker
uses, and the only path by which `consumes` is reachable, since it has
no Cookfile surface syntax. A sibling `data.txt` accompanies the fixture
so the declared input exists on disk.

The negative counterpart is `add-test-consumes-bad-glob`: an unparseable
pattern MUST be rejected at register phase rather than silently matching
nothing, since a `consumes` that matches nothing would narrow the key to
the point of replaying a stale pass.
