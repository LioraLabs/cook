# gather-source-trailing-nonglob

§8.2 Form 2 constraints 1 and 3, which share one code path and one diagnostic:
arity is exactly one bare source, and the trailing items are `STRING*`, so
neither a second bare source (this fixture) nor an exclusion
(`gather sources !"include/bad.h"`) is admitted after it.

The exclusion variant is deliberately not a separate fixture: it is the same
rejection from the same branch, and `error.txt` is a substring match, so a
second directory would pin nothing the first does not. (CS-0231.)
