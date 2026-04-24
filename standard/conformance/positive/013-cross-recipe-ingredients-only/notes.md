Pins § 5.4.1 passthrough: `headers` has no cook step, so its output list at register time is the resolved `ingredients`. `list_headers` consumes that list via the `{headers.stem}` accessor and fans out to one work unit per header.

The parser-only conformance harness can only pin the parsed AST shape; the passthrough semantics and the `list_headers → headers` cross-recipe edge are runtime concerns tested at the `cook-luagen` / `cook-engine` layer.
