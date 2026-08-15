CS-0239 §8.2 Form 3, constraint 3. A recipe gathering its own outputs is the
degenerate cycle: the member set would be the output of the units the member
set registers.

It is caught by name rather than by the general cross-recipe cycle rule
(§10.6) because a self-edge is decidable from one recipe and deserves a
diagnostic that says so.
