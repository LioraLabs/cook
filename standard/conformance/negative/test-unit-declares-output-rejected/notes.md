CS-0185 §22.4. A `step_kind = "test"` unit declares no outputs.

The empty output list is what makes a unit's cache hit replay a recorded
outcome rather than restore artifacts; `step_kind` is what makes it a test.
Neither is inferred from the other, so an output here is an author error and
not a silent reclassification into a producing unit.

Register-phase, not syntactic: the tree-sitter harness records this as a
semantic skip.
