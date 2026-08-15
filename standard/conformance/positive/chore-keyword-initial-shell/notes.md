Pins CS-0233 (§{chores.keyword-initial}): a chore body bans three step KINDS,
not the three words that open them. Every line here is a `shell_command`.

- `test -f src/canvas.ts` — the keyword `test`, but the remainder is not a
  body (`{` / `>{`), so it is not a `test_step`.
- `[ -f src/canvas.ts ]` — the other everyday conditional spelling. It never
  matched a keyword; it was unreachable because the emitter wrapped it in a
  long-bracket literal that its own trailing `]` closed early (CS-0233).
- `test "$X" = ""` — a quoted remainder is a `gather` / `cook` shape only. A
  `test` step's sole operand is its body, and `test "cmd"` is not a
  `test_step` in any position (§{steps.test}).
- `cook build` — invoking the `cook` binary from a chore. The remainder is
  not an output pattern, so it is not a `cook_step`.
- `gather -x logs` — the remainder is neither a glob, an exclude (`!"…"`), nor
  a lone bare identifier, so it is not a `gather_step`. The exclude spelling is
  pinned in the other direction by `negative/chore-gather-exclude-glob`.
- bare `test` — no remainder at all, so no step syntax.

The ban itself is pinned by the negative corpus and by the `cook-lang`
parser tests: `test { … }`, `test >{ … }`, `cook "out" { … }`,
`cook (expr) >{ … }`, `gather "src/*.c"`, `gather !"build/*"` and
`gather members` are all still rejected inside a chore body.
