# gather-source-not-first

§8.2 Form 2 constraint 2: a bare gather source MUST come first. App. A.4 spells
the arm `probe_ref STRING*`, so a quoted item may not precede the source.

Pinned because the accept side (`gather sources "include/*.h"`,
`positive/068-gather-named-files`) is easily mistaken for "the two forms mix
freely" — the Standard asserted exactly that mutual-exclusion error until
COOK-486. The rule is ordering and arity, not exclusion, and only a negative
shows the ordering half. (CS-0231.)
