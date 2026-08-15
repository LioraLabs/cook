Pins §22.5.2 and §22.5.9 (CS-0226): the `envs { NAME, … }` probe producer is
removed, and the rejection MUST name its replacement.

The expected substring is the whole diagnostic, not just the "was removed" half,
because the migration advice is the part the Standard mandates: 22-probe-units.mdx
requires "a removed-keyword diagnostic naming an ordinary shell probe such as
`lines { echo "$NAME" }` as the replacement", and A-grammar.mdx §A.5 requires the
same of every retired producer spelling. A diagnostic that kept the rejection and
dropped the advice would satisfy a bare "does it error" test and still strand every
author holding a pre-v2 Cookfile.

The two names are load-bearing: the replacement is rendered from the operand list,
so `SDKROOT` and `CC` appearing in the expected `lines { … }` body proves the
diagnostic reads the author's own names rather than printing a fixed example.

COOK-485.
