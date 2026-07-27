# cook-contracts

`cook-contracts` defines Cook's shared language as plain data types and pure
rules.

It owns the contracts exchanged between Cook crates: captured work, recipes,
cache metadata, probes, registration names, and canonical value rendering.
Each concept lives in its own directory module, while the crate root remains an
index that preserves the established public imports.

It does not own filesystem or environment access, process execution, VM policy,
cache backends, scheduling, or runtime orchestration. Those mechanisms belong
to the crates that consume these contracts.
