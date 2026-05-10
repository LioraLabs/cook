Pins the surface syntax for the §5.4.1 plate-passthrough rule. The
runtime contract — that `$<greet>` substitutes to `Cookfile` (the
plate's input list) and not the empty string — is enforced by
cook-luagen / cook-register unit tests; the conformance harness here
just confirms parse + codegen succeed for the surface form.
