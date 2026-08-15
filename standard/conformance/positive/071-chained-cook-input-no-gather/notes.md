Pins the chained-step input source: the second `cook` step names `$<in>` with
no `gather` anywhere in the recipe, and its input list is the preceding `cook`
step's collected outputs (§8.2, §8.4.1).

Both outputs are literal, so the second step is many-to-one (§8.4.1): `$<in>`
is the whole collected set, not an iteration item. That is the point — §9.3's
restated rejection (CS-0232) turns on whether a source exists, and cardinality
is §8.4.1's separate question, per CS-0224's "`gather` does not select
cardinality".

This is the one source in that rejection no other fixture pins.
`013-cross-recipe-gather` pins the dep-driven output pattern;
`negative/command-input-without-gather` pins the floor — `$<in>` with no source
at all. A future narrowing of `$<in>` back to gather-only, which is what §9
said before CS-0232, turns this case red.
