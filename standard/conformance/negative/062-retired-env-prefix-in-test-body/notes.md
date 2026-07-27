Pins CS-0184 § 10.2.4 position independence, using the CS-0172 retired-prefix
diagnostic as the probe.

The identical token in a `cook` body has produced the retired-prefix migration
message since CS-0172. In a `test` body it produced `no config block declares
'env.HOME'; declare it with var.env.HOME = ...`, because the test-body expander
answered what the token meant without consulting § 10.2's resolution function.
The advice was not merely different, it was impossible: `env.HOME` cannot be
declared as a variable name.

Parses cleanly; rejected at codegen, with the same diagnostic either body kind
produces.
