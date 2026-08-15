CS-0239 §8.2 Form 3, constraint 3's second half. A recipe gathering its OWN
outputs is caught by name (`gather-recipe-self`); a longer cycle is not
decidable from one recipe and falls to the general cross-recipe cycle rule of
§10.6, which every name reference already feeds.

That delegation is the thing worth pinning. `gather $<b>` establishes its edge
through the ordinary `requires` inference, not through a mechanism of its own,
so a two-recipe cycle must be rejected by the same pre-walk that rejects a
`: dep` cycle — with the same diagnostic. If the new form ever grew a private
edge channel, this fixture is what would notice.

Register-phase, because the cycle is a property of the assembled graph rather
than of any one declaration: both Cookfile halves parse cleanly and each
recipe is individually well-formed.
