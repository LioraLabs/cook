App. A grammar `cook_mods ::= ("seal" probe_ref+ | "unseal" probe_ref+)* share_mod?`:
a disposition group takes one or MORE refs, and a bare `BARE_PROBE_KEY` is a
valid ref in any position within the group — not just the first.

Covers the gap that let a tree-sitter regression sit unnoticed: every prior
corpus case used either a single bare ref or `bare ns:prefixed`, so a group
carrying two consecutive bare refs was never exercised on a cook step.
