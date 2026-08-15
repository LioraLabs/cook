# cook-disposition-seal-tools-probe

Pins the Session-3 decision (§8.4.3): a tool determinant folds by sealing a
top-level `tools` declaration by name (no `tool.X` inline ref form). The
`tools toolchain` declaration is sealed via `seal toolchain`, folding the toolset
fingerprint into the cook's key. (COOK-172, CS-0117.)
