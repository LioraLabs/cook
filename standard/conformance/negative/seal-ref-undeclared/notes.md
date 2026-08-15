Pins §8.4.3.1 rule 4 (CS-0235): a `seal` ref that resolves to no declaration is
rejected in the vocabulary of the `seal` step the author wrote.

The rejection lives at register time — resolution is deferred to end-of-pass by
§22.5.6 — so this is a `register_error.txt` fixture rather than an `error.txt`
one.

What the fixture defends is the negative half. A seal ref is unioned into the
unit's consumer `probes` list so the sealed probe is scheduled ahead of the unit,
and before CS-0235 the ref inherited §22.5.6 rule 1's mandated sentence with it:
"unit 'main.o' lists probe key 'no-such-decl' in `probes` but no such probe was
declared". Every noun in that sentence is `cook.add_unit` API surface. The author
wrote three words of Cookfile, none of them `probes`, and was told about a Lua
field they had never seen. The expected substring names `seal` and the
declaration kinds a ref may resolve to, which is the whole content of the fix.

§22.5.6's own sentence is unchanged and still mandated for the `probes` field;
its guard is the reference implementation's `unresolved_probes_key_errors`.

COOK-484.
