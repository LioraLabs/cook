# cc-find-cmake-opt-in

Locks §9.2.3.8 v0.3 `FindOpts.cmake = true`. The opt-in lifts cmake-compat
to position 3 in the chain (after curated, before pkg-config) and skips
pkg-config for that call. Cookfile-level lock only; runtime ordering verified
by busted.
