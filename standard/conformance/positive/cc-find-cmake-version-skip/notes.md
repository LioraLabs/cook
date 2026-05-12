# cc-find-cmake-version-skip

Locks §9.2.3.8.1 step 1: cmake-compat MUST return `outcome="skip"` when
`opts.version` is set (legacy `cmake --find-package` cannot honour version
constraints). Cookfile-level only; the skip behavior is verified by busted.
