CS-0228: `seal` inside a probe body names the probe's determinant declarations,
which join its `inputs.requires` list. A bare operand names an upstream probe
key directly; a quoted operand, which is what this fixture pins, names a path
that desugars to an anonymous `files` probe whose key joins that same list.
