Pins the authoring shape CS-0167 (§28.3.5.1) makes buildable: a `cc.lib`
target whose source set contains repeated file *basenames* — here
`bignum.cpp` twice, plus `dictionary_compression.cpp` alongside
`dictionary/compression.cpp` — with **no** caller-supplied `output`
override on any source.

Under the pre-CS-0167 derivation (`build/obj/<target>/<stem>.o`) this
Cookfile was legal to write but impossible to build correctly: both
`bignum.cpp` units aliased onto one object path, one silently won, and the
archive shipped without the other translation unit. The `dictionary` pair
is the case that also defeats the obvious "sanitise the path" repair —
separator folding maps both onto `dictionary_compression.o`, which is why
§28.3.5.1 requires injectivity by construction rather than by transform.

Parse-only verification, per the corpus's cc-* convention (see
`cc-bin-cpp-source`). The derivation itself is a runtime property and is
asserted in the busted suite at
~/dev/cook-modules/cook_cc/spec/cc_spec.lua, which pins that these four
sources produce four distinct object paths and that cc.lua and
compile_db.lua agree on every one of them.
