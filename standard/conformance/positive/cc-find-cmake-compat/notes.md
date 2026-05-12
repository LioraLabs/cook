# cc-find-cmake-compat

Locks §9.2.3.8 v0.3 normative chain stage `"cmake-compat"`. The Cookfile
parses identically to any other `cc.find` call site; the strategy slot is
exercised at runtime by busted in cook-modules/cook_cc/spec/. Execute-mode
locking is filed under SHI-210.
