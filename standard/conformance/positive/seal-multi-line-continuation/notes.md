CS-0238 extends the CS-0078 continuation rule to `seal_step`. A physical line
whose first non-whitespace token is `"` or `!"` belongs to the preceding `seal`,
exactly as it does for `gather` and `cook`.

All three quoted operands — including the exclusion, which arrives on a
continuation line — collect into the ONE anonymous `files` determinant of the
first `seal` step. Rule 1 of §8.4.3.1 ("exclusions require an include") is asked
over the step, not the physical line, so the exclusion here is satisfied by an
include two lines above it.

The `@seal:` name is CS-0236's content fold over the operand list, carrying
neither the owner nor a line number. So a wrapped `seal` and the same operands
written on one line name the SAME determinant, and neither the wrap nor the
lines it shifts below it move a cache key.

The fifth line is `seal toolchain`, a bare operand at the start of a line. It
does NOT continue the first step — the continuation trigger is the leading
quote, not the operand kind — so it dispatches as a second `seal` step, and the
two union per rule 2. That is the stacked-`seal` idiom, still meaning what it
meant before CS-0238.

The rule bans a bare key only in the FIRST position of a continuation line. A
line already admitted by a leading quote is read by the rules that apply on the
keyword's own line, so `"b.ts" toolchain` would have joined the first step.
