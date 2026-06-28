# cook-disposition-seal-tools-probe

Pins the Session-3 decision (§8.4.3): a tool determinant folds by sealing a
`tools` probe BY NAME (no `tool.X` inline ref form). The `tools { cc, ld }`
probe `toolchain` is sealed via `seal toolchain`, folding the toolset
fingerprint into the cook's key. (COOK-172, CS-0117.)
