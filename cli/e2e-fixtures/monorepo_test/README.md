# `examples/monorepo_test/`

Workspace-shaped fixture pinning monorepo test discovery. Three imported
Cookfiles (`apps.web`, `apps.api`, `shared`) each contain test recipes.

Bare `cook test` at this root discovers all of them. Namespace and
recipe scopes are pinned by the walkthrough.

Run: `cook test` (after Phase 4) at this directory.
