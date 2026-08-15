Pins §8.1 step-dispatch rule 3 (CS-0226, CS-0235): `envs` + separator in
recipe-body position is rejected as removed, and the rejection MUST carry the
removal's own migration advice rather than rule 7's.

The negative half of the assertion is the point of the fixture. Before CS-0235
this line matched no cascade rule and fell through to rule 7, so the author was
told "loose shell commands are not allowed in a recipe body (CS-0134) — move it
into a `cook` body or a chore". That advice is not merely unhelpful, it is wrong:
`envs` is removed from the language, so moving the line anywhere reproduces the
same failure, and the diagnostic named neither the removal nor its replacement.
A fixture pinning only "does it error" would have passed against that message.

The twin of `probe-producer-envs-removed`, which pins the same removal in the
probe-producer position. Both spellings render the replacement from the author's
own operand names — `CC` and `CFLAGS` here — from one shared renderer, so the
two positions cannot drift into saying different things about one removal.

The `gather` and `cook` lines are present so the rejection is reached in the
shape a migrating pre-v2 Cookfile actually has, rather than in a recipe whose
only body line is the removed keyword.

COOK-484.
