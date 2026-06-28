# cook-disposition-seal-envs-probe

Pins the Session-3 decision (§8.4.3): a seal ref is always a probe name, and
an unconsumed-environment determinant folds by declaring an `envs` probe and
sealing it BY NAME (no `env.X` inline ref form). The `envs { CFLAGS, LDFLAGS }`
probe `flags` is sealed via `seal flags`, folding the env-set fingerprint into
the cook's key. (COOK-172, CS-0117.)
