Pins the § 5.5 bare-name cross-recipe reference. `app`'s using-string contains `{libmath}`, which per the Standard expands at register time to the space-joined output list of `libmath`.

The conformance harness is parser-only, so this fixture asserts that the reference parses verbatim into the step template. The runtime behaviour (lowering to `cook.dep_output("libmath")`, cross-recipe dep edge `app → libmath`) is covered by `cook-luagen` unit tests.
