# Preserved modification times (CS-0254)

This Cookfile declares a content-copying build. The parser corpus verifies its
syntax and AST only; parsing does not prove runtime cache validation.

Runtime contract: after building with `input.txt` containing `first`, replace its
bytes with `other` (equal length) or `different length`, restoring its original
modification timestamp. Building again must copy the new bytes. Independently
corrupt `output.txt` with either replacement while restoring its timestamp:
the next build must restore or regenerate the original expected output.
An unchanged build remains cached; a same-content touch does not recompile.

The retained public cache-decision regressions execute these real filesystem
mutations for declared inputs, discovered inputs, loaded modules, and outputs:

```sh
cd cli
cargo test -p cook-cache --lib preserved_mtime_
```

`check::tests::preserved_mtime_{same,different}_length_{input,module,output,discovered}`
assert changed-input/module causes or output drift. They fail semantically with
the old mtime-only validator and pass with strong local metadata validation.
`same_bytes_refresh_strong_evidence_and_absent_evidence_is_conservative` covers
same-content touches and records without strong evidence; existing marker and
record-disposition tests preserve their distinct rules.
