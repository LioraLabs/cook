# cc-frameworks-on-link

Locks §9.2.3.7 `LinkOpts.frameworks` Cookfile surface — recognised as an
option key on `cc.bin`/`cc.lib`/`cc.shared`. Runtime emission of
`-framework <name>` is verified by cook_cc/spec/cc_spec.lua.
