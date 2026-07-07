Pins §22.5.7's native-body consumption surface: a probe-value sigil
(`$<sys:os>`) written in a native `cook`-step shell body parses as ordinary
shell text, unchanged by the presence of a colon in the IDENT — desugaring to
the register-time capture rewrite happens later, not at parse time.
Regression anchor for COOK-187/CS-0122 (luagen previously lowered such
references to a function-valued `command`, which `cook.add_unit` silently
coerced to `""`).
