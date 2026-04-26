# Design: Top-level `Cookfile` for the Cook project

**Date:** 2026-04-26
**Status:** Design — pending implementation plan
**Standard change ID:** None. This is project-tooling work; it does not modify the Cook Standard, the corpus, or any normative chapter.
**Scope:** Add a top-level `Cookfile` at the repo root that captures the developer workflows established in `2026-04-26-cli-standard-conformance-workflow-design.md`. Recipes are the canonical front door for verification (`cook check`), the mechanical rituals around Standard cuts (`cook bump-claim`, `cook retag`), and self-installation (`cook install`).

## 1. Motivation

The conformance workflow that just shipped names a handful of common operations: building, testing, running the conformance harness, building/linting the Standard's MDX, running the backwards-conformance script, bumping `COOK_STANDARD_VERSION` and its mirrors, force-moving a `cs-standard/vX.Y` tag. Today these live as bare commands in `CONTRIBUTING.md` and `CONFORMANCE.md`. A top-level `Cookfile` collects them as named recipes so the canonical invocation is `cook <recipe>` instead of "look it up in CONTRIBUTING.md and copy-paste."

Two secondary benefits:

- **Dogfooding.** Cook gets exercised on a real, non-toy project (its own repo). Every developer command is a Cook recipe, so ergonomic friction surfaces during normal use rather than only in toy examples.
- **Living documentation.** A reader who wants to know "what does this project actually do?" can read one short Cookfile.

## 2. Non-goals

- **Replacing cargo or pnpm.** Cargo still owns Rust builds; pnpm still owns the Standard's Astro build. The Cookfile shells out to both.
- **Authoring rituals (cut procedure).** The cut procedure in `CONTRIBUTING.md` requires prose authoring (App. D Versions index entry, CS body `**Version:**` lines). Half-mechanizing it produces worse outcomes than a checklist; out of scope for this Cookfile. `cook bump-claim` and `cook retag` cover the post-cut mechanical follow-up.
- **First-time bootstrap.** Any build tool that installs itself faces the same chicken-and-egg: the first install cannot use the tool. The Cookfile assumes `cook` is already on PATH; the README documents the one-time `cargo install --locked --path cli/crates/cook-cli`.
- **Cross-platform polish.** The recipes assume a Unix-y environment (Linux is the project's primary target). `cook install` relies on the OS allowing replacement of a running executable's file (Linux/macOS yes, Windows no). Windows support is not a goal here.
- **Build artifacts.** No `cook "<output>" using "<command>"` recipes — there are no Cook-managed outputs in the project. The Cookfile is task-runner shape, like `examples/monorepo/Cookfile`.

## 3. Recipes

Eleven recipes total. Names use `kebab-case` for multi-word recipes (matching examples like `compile-commands`, `test-vec`, `run-tests`).

### 3.1 Verification (7)

```
recipe build           # cargo build of the workspace
recipe test            # cargo test of the workspace
recipe conformance     # cargo test -p cook-lang --test conformance (the gate)
recipe version         # build cook + invoke `cook --version` to surface the claim
recipe standard-build  # cd standard && pnpm build (validates MDX renders + bare-ref-lint passes)
recipe standard-lint   # cd standard && pnpm lint:keywords
recipe against-tag     # standard/scripts/check-conformance-against-tag.sh "v$VERSION"
```

`against-tag` reads `VERSION` from `--set VERSION=X.Y`, defaulting to the contents of `standard/VERSION`. The script prefixes with `v`; the recipe passes `"v$VERSION"`.

### 3.2 Mechanical rituals (2)

```
recipe bump-claim      # rewrite COOK_STANDARD_VERSION + 3 mirrors (README × 2, CONFORMANCE.md)
recipe retag           # git tag --force "cs-standard/v$VERSION" HEAD
```

Both default `VERSION` to `standard/VERSION`'s contents. `cook bump-claim` is meant to be run immediately after a cut commit lands; `cook retag` after `bump-claim` if the dump format changed (per the operating rule added to `CONTRIBUTING.md`).

`bump-claim` rewrites four files:

1. `cli/crates/cook-lang/src/lib.rs` — `pub const COOK_STANDARD_VERSION: &str = "X.Y";`
2. `cli/crates/cook-lang/README.md` — `claims **Cook Standard vX.Y**`
3. `cli/crates/cook-lang/CONFORMANCE.md` — `claims **Cook Standard vX.Y**`
4. `README.md` — `claims **Cook Standard vX.Y**`

The recipe shells out to `sed` for each file. Idempotent: running twice with the same VERSION is a no-op.

### 3.3 Self-install (1)

```
recipe install         # cargo install --locked --path cli/crates/cook-cli
```

Behavior on Linux/macOS: cargo overwrites `$CARGO_HOME/bin/cook` while the running cook process keeps its mapped inode. Recipe completes; the next invocation picks up the new binary. On Windows this would fail; not a target.

### 3.4 Umbrella (1)

```
recipe check: build test conformance standard-build standard-lint
```

Empty body. The recipe-deps mechanism runs each dep recipe before `check`'s own (empty) body. Use case: pre-commit smoke test, single command.

## 4. Argument convention

Recipes that take a version use `--set VERSION=X.Y` (just the dotted form, no `v` prefix). Default: read `standard/VERSION` as a single line. Override pattern:

```
cook bump-claim --set VERSION=0.4
cook retag --set VERSION=0.3
cook against-tag --set VERSION=0.1
```

In recipe bodies, the resolved version is computed at the top of the body via shell:

```
recipe bump-claim
    @VERSION="${VERSION:-$(cat standard/VERSION)}" && \
        sed -i "s|...|...|" cli/crates/cook-lang/src/lib.rs && \
        ...
end
```

The `@`-prefix marks the line as interactive shell so output streams unbuffered.

Single-line vs multi-line bodies: short recipes inline the command. Recipes that span multiple shell commands either chain with `&&` on one line, or split into multiple recipe-body lines (each line is one shell step in Cook's recipe-body model).

## 5. Style choices

- **Flat.** No `config` blocks, no `use` modules. Every recipe is self-contained.
- **`@`-prefix used freely.** Cook's default is non-interactive shell with output capture. `@`-prefix gets streaming output, which matches what cargo, pnpm, and shell scripts expect from a terminal session. Verification recipes that produce long, useful output (cargo test, pnpm build) use `@`.
- **No `cook "<out>" using "<cmd>"` build steps.** The cargo and pnpm subprocesses manage their own outputs.
- **Top-of-file comment** documents two common invocations:
  - `cook check` — pre-commit smoke test.
  - `cook bump-claim && cook retag` — post-cut sequence after a Standard version moves.
- **Comments are minimal.** Recipe names and bodies are self-documenting; comments live only at the top of the file and at non-obvious recipes (e.g., flagging that `against-tag` reads `VERSION` from `--set`).

## 6. README update

A one-line change to the project root `README.md` immediately after the existing "claims **Cook Standard v0.2**" line:

> First-time setup: `cargo install --locked --path cli/crates/cook-cli`. After that, `cook install` updates in place; `cook check` runs the full verification suite.

This is the only documented bootstrap. `CONTRIBUTING.md` may pick up cross-references to the Cookfile recipes in a follow-up; out of scope for this design.

## 7. Verification

After the Cookfile lands and `cook` is installed:

```
cook check         # all 5 verification deps pass
cook version       # prints `cook 0.1.0 (Cook Standard v0.2)`
cook against-tag   # backwards-conformance against the current standard/VERSION tag
```

`cook bump-claim` and `cook retag` are not run as part of routine verification — they're tested by inspection (read the recipe body, eyeball the sed expressions) and by exercising them on the next real cut. As a safety net, the implementation plan adds a sanity-check step that runs `cook bump-claim --set VERSION=0.2` (the current claim) and confirms the diff is empty (idempotent on the current state).

## 8. Out-of-scope follow-ups

- **`cook cut` recipe.** Could scaffold the cut procedure, but the prose-authoring requirements push it past mechanical-ritual territory. Deferred until/unless someone wants it.
- **Self-update via `cook install` reading from a remote.** The current recipe is local-source-only. A future `cook install --remote` could pull from a release URL; not relevant pre-1.0.
- **Cookfile validation in CI.** When/if CI is added (the project currently runs no automation per `project_git_hosting.md`), a `cook check` invocation is the natural single-command entry point.
- **Cross-references from CONTRIBUTING.md to Cookfile recipes.** Worth doing eventually so the doc isn't a parallel command list.
