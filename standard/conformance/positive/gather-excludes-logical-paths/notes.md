# Logical gathered-path exclusions (CS-0255)

The parser corpus proves this anchored/include/exclude syntax and AST. Runtime
selection is covered by `cook-register` context tests, not by parsing alone.

`unrelated_physical_alias_cannot_exclude_a_logical_gathered_path` creates a real
`src/file.txt` and an unrelated Unix filename containing a literal backslash.
Excluding `*.txt` must retain the gathered `src/file.txt`, whose logical path has
an intervening directory separator. The old separately expanded exclusion set
incorrectly removed it. Both public Lua resolution and recipe context are checked.

`exclusions_do_not_traverse_unrelated_trees` observes directory opens with Linux
inotify and proves that excluding node_modules does not enumerate it when the
include selects another file. It fails against the old expansion path without a
timing threshold. `recipe_and_public_gather_preserve_groups_warnings_and_fresh_membership`
covers grouping, duplicates, warnings, source creation and removal. Cache gather
tests compare ordinary matching with filesystem expansion across anchors, hidden
files, symlinks, and existence-sensitive parent components.

Run `cargo test -p cook-register --lib context::tests` and
`cargo test -p cook-cache --lib gather_glob_tests` from `cli/`.
