Pins two `test` steps with no trailing modifiers (CS-0135: `as`/`timeout`/
`should_fail` were removed in v1.0). A negative assertion is expressed by
inverting the check in the body (`test { ! ./mustfail }`) rather than a
`should_fail` modifier.
