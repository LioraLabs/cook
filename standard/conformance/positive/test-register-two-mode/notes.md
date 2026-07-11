Pins register-phase success (`register_ok.txt`) for the two surviving
`test`-step iteration modes (CS-0135, Standard §22.4):

- `suite.test { ./$<in> }` — a one-to-one fan-out test whose source is the
  preceding `cook` step's outputs (`t/*.sh` → `build/$<in.stem>` →
  per-file `test`), lowering to one `cook.add_test` call per fan-out member.
- `naked.test { echo ok }` — a source-less, single-unit "naked" test with no
  preceding `cook` step and no `ingredients`, lowering to exactly one
  `cook.add_test` call.

Both recipes must register end-to-end without error under the reshaped
`cook.add_test` surface, which no longer accepts `name`/`timeout`/
`should_fail` (those fields were removed from the `test` step along with
the `plate` step kind).
