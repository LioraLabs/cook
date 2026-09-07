CS-0256: a chore composes a faithful command for an existing recipe through the
public `cook.child_command` helper and the existing uncached interactive unit.

The parse golden establishes syntax only. Executable coverage is retained in
`cook-cli/tests/child_invocation.rs` (wrong PATH, non-default entry, member/preset
scope, literal overrides, policy refresh, publish-off and failure) and
`cook-cli/tests/chore_process_ownership.rs` (real terminal input, SIGINT and
spawned-descendant termination). `cook-register` capture tests cover rejection
outside a chore and when an embedding host omits invocation context.
