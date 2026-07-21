CS-0159 rule 10: `local` / `pinned` / `nondet` state a fact about an output
artifact, and a test produces a pass/fail record rather than artifacts, so a
share_mod is rejected in a `test_mods` position. A test takes the INPUT half of
the tail (seal/unseal) only.
