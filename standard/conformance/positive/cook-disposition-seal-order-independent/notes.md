# cook-disposition-seal-order-independent

Pins §8.4.3 rule 2 (declarative, not positional). The recipe-level `seal host`
is written *after* the `cook` step, yet still folds into the cook's effective
seal set (`seal=["host"]`). Scope is determined by where the directive is
written (recipe-body step = recipe baseline), never by what textually follows
it; the per-unit set is computed at recipe finalize, not accumulated positionally.
(COOK-172, CS-0117.)
