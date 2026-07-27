Pins CS-0184's refusal clause: a position that cannot honour a resolution MUST
refuse it rather than substitute a value the phase cannot have.

A `test` unit's command runs verbatim, so substituting a probe-value reference
(§ 22.5.7) would fall to register time, where a demand-scheduled probe has not
been produced and answers with no value. Before CS-0184 this token never
reached the resolver at all: the test-body expander hand-rolled its own
dispatch chain with no probe arm, so `$<sys:os>` fell through to the declared-
variable step and reported `no config block declares 'sys:os'` — advice that
cannot be followed, since `sys:os` is not a legal variable name.

Parses cleanly; rejected at codegen. The same diagnostic covers the
`ingredients <probe>` fan-out test path, which has always refused.
