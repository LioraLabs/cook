Pins CS-0187's required diagnostic for the retired `file:` prefix.

Removing the namespace is not by itself enough to make the retired form fail
cleanly, which is why the entry requires a diagnostic rather than recommending
one. `file:` was dispatched AHEAD of the probe-value colon discriminator, so
with that dispatch gone two different rules claim what is left:

  * `$<file:tokens.css>` — every character is in the `bare_ident` set, so it
    resolves as a probe-value reference and reports an undeclared probe key
    `file:tokens`: a diagnostic about a probe the author never wrote. That is
    the shape this fixture uses.
  * `$<file:templates/*.html>` — `/` is in no ident production, so §{phl.token}
    strict-bails and the whole sequence stays literal shell text, reaching the
    step's command line unsubstituted. Not pinnable as a negative: it parses,
    and it is the strict-bail rule working as specified.

The corpus pins the first because it is the one a diagnostic can catch, and
because the wrong-probe-key message is what an author would otherwise be left
holding. The same treatment CS-0172 gave the retired `env.` prefix, pinned by
`062-retired-env-prefix-in-test-body`.

The replacement is a `files` probe (CS-0148) sealed on the unit that reads it;
the diagnostic names it, so the fixture's expected text is deliberately the
short stable fragment rather than the whole sentence.
