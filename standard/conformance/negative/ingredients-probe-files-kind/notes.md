§22.5.2 / §22.5.9 (COOK-353): a `files { … }` producer is seal-only. Its
value is a JSON object keyed by path, so it can never resolve to the array an
`ingredients <probe>` driver iterates, and naming one in driver position is a
register-phase rejection.

The rejection is not new — §22.5.9 has always required an array source — but
the Standard left the consequence for `files` implicit, and the reference
implementation reached it by a worse route: the reserved `@files-manifest`
produce sentinel was not intercepted on the `ingredients <probe>` pre-pass, so
it was handed to the Lua VM and the case died with
`syntax error: unexpected symbol near '@'` rather than a diagnostic.

The expected error names the fix (`seal`), not just the shape mismatch: the
actionable fact is the producer KIND, not the JSON type.
