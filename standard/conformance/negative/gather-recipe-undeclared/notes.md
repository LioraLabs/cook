CS-0239 §8.2 Form 3, constraint 2. A `gather $<NAME>` source whose name
resolves to no recipe is rejected.

The rejection is codegen-phase, not parse-phase, and that is forced by
§10.2.4: name references are position-independent, so a forward reference is
legal and the parser cannot know the recipe set while it is still building it.

The diagnostic says "in this Cookfile", not "in scope", and the difference is
load-bearing: §8.2 Form 3 constraint 2 scopes the source to the current
Cookfile, and a QUALIFIED reference (`$<lib.gen>`) is refused by a separate
message that names the position rather than calling the recipe undeclared
(§10.2.4; COOK-512). Reporting both as "not in scope" would tell an author
whose import is perfectly good to go looking for a typo.
