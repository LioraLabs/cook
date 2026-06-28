# cook-disposition-unseal-one-unit-only

Pins §8.4.3 rules 4–5: a trailing `unseal` adjusts one unit only.
`effective(unit) = (baseline ∪ trailing seals) − trailing unseals`.
Baseline `{a, b}`; `x.o` carries trailing `unseal a` → `seal=["b"]`, while
`y.o` (no tail) keeps the full baseline `seal=["a", "b"]`. Per-unit isolation:
the unseal on one cook does not affect the other. (COOK-172, CS-0117.)
