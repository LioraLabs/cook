Pins CS-0206, and it exists because review found the two implementations
disagreeing about this exact Cookfile.

App. A.5's `path_segment` is `/[^\s\n\/]+/`, which admits no whitespace, and
`tree-sitter-cook` matched the Standard. The Rust lexer's argument splitter
honours the quoted form — so that a path CAN be quoted at all — and that
leniency let `use h "./lua/my helpers.lua"` parse in one conforming reader and
fail in the other. A Cookfile whose meaning two readers disagree about is worse
than either answer, so `layout::normalise_use_path` now refuses whitespace at
both doors and this fixture pins the agreement.

Quoting a `use` path is still supported; it just does not buy the ability to
put a space in one.
