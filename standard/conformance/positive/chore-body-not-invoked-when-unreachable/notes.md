Pins Standard §{chores.speculative} / CS-0218: a chore body is invoked only when
the chore is the dispatch target or is reachable from it.

The register harness registers every positive fixture with **no dispatch
target**, which is the pass with an empty reachable set. Both chores the module
registers are therefore speculative here by construction, and the assertion is
the harness's own success criterion: the pass must complete. No new harness
machinery, no golden file, nothing to keep in sync.

The chores are registered through `cook.chore` rather than written as surface
`chore` blocks, and that is load-bearing rather than incidental. A surface chore
body admits no register-phase step (§{chores.body}): every line of it becomes a
work unit, so an `error(...)` written as a `>` step raises at execute phase, in
a unit that a chore nobody invoked never runs. A fixture shaped that way passes
whether or not the register phase invokes the body — which is to say it pins
nothing. An earlier draft of this one was exactly that, and passed with the
amendment reverted.

Both a paramless and a parametric chore are present. Only the paramless one
changes behaviour under CS-0218; the parametric one has been skipped since
Note 7.5.1.1. Pinning both is what makes this a fixture about the rule rather
than about the arm that was wrong, and it fails if a later change reintroduces
a predicate over `__params`.

`app` carries no dependency on either chore. An edge would put the chore in the
reachable set, and the fixture would then be pinning the opposite rule.

Before CS-0218 this fixture fails: `verb.scaffold` declared no parameters, so
its body was invoked on every register pass and the `error` fired.
