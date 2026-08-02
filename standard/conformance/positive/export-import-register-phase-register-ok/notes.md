§{lua.cook-exports-imports}, CS-0200. Pins the register-phase round trip that
`cook_cc`'s transitive-link walk depends on: `record_export` publishes a
target's outward-facing fields and `transitive.lua` reads them back while
recipes register, because the product of that walk is the compile and link
command lines, which must exist before any unit is captured.

Until CS-0200 this section claimed **Phase: Both** and §{mods.lifecycle.rehydration}
required a register-time export to be observable to an execute-phase
`cook.import`, both "[Pinned by CS-0071]". No fixture existed for either
claim. The implementation built an empty per-VM table on the worker and never
seeded it, so an execute-phase import returned `nil` for every register-time
export — which §{lua.cook-exports-imports} simultaneously permitted, by
allowing "a per-worker scratch store" with a minimum bar of round-tripping
"within a single worker VM". The two clauses could not both be satisfied.

CS-0200 withdrew the execute-phase half rather than implementing it, and this
fixture exists so the surviving half is pinned by something that runs.
