# Cook Module Registry MVP — `cook pull`

> **Status: Superseded by `2026-05-08-luarocks-modules-design.md` (SHI-176 Phase 3+4). The `cook pull` subsystem this spec describes was deleted in Phase 4 (M4.3) and replaced by `cook modules` backed by LuaRocks. This document is preserved for design history.**

Date: 2026-05-07
Status: Superseded — see header.
Standard impact: none. The Standard's `cook_modules/` resolution semantics (Appendix B-rationale) are unchanged; `cook pull` only writes files into a project-local directory whose existence and resolution rules already exist.

## Context

The Cook Standard establishes `cook_modules/` as a project-local, sibling-to-Cookfile directory and explicitly rejects a global registry on locality grounds: "A Cookfile's modules are the ones it ships with; they travel with the project, they are the ones code review sees, and they are the ones `git clone` reproduces." (App. B-rationale, §modules.)

That stance is correct and is preserved by this design. What is missing today is a frictionless way to **bootstrap** a project's `cook_modules/` directory from a curated set of community modules — the user wants to type `cook pull cpp` in a fresh directory and end up with a working `cook_modules/cpp/` next to their Cookfile, ready to commit. Once pulled, the module is part of the project; `git clone` reproduces it; the Standard's locality contract is unaffected.

This MVP introduces a single CLI subcommand, `cook pull`, that downloads named module subtrees from a configurable git registry over HTTPS and writes them into the project-local `cook_modules/` directory.

## Goals

- `cook pull <name>` populates `./cook_modules/<name>/` from the configured registry.
- Multiple names per invocation: `cook pull cpp rust`.
- `--all` pulls every module the registry exposes.
- `--list` enumerates available modules without writing.
- Conflict-aware writes: interactive `y/n/a/q` prompt on overwrite, `--force` to suppress.
- Trust-on-first-use (TOFU) prompt before pulling from any registry URL the user has not previously consented to. Consent is per-URL, persisted to the user's config dir.
- Zero ambient state: re-running the command from a clean checkout reproduces the same files. There is no lockfile, no sidecar metadata, no `.cook-pull` marker.

## Non-goals

- **Versioning, lockfiles, or pinning.** The registry is a single mainline branch; what you pull is HEAD. Reproducibility comes from the user committing `cook_modules/` to their own VCS, not from a registry-side pin.
- **Integrity beyond HTTPS.** No checksums, signatures, or content hashes. HTTPS-to-known-host is the trust boundary for v1. (See §10 for when this needs to change.)
- **Authentication.** Public registries only. No bearer tokens, no basic auth, no SSH keys. A private-registry story is future work.
- **Dependency resolution.** A pulled module that itself `require`s another module is the user's responsibility to also pull. v1 has no manifest, no `cook_modules.toml`, no dependency graph.
- **Update / sync command.** Re-running `cook pull <name>` and answering the prompts is the only update path. No `cook pull --update`, no diff view.
- **Nested registries, scopes, or namespaces.** A module's identifier is a single bare name. A registry has a flat list of modules; collisions across registries are out of scope because v1 has one registry at a time.
- **Cookfile detection.** Pulling into a directory with no Cookfile is allowed; `cook pull` is a valid bootstrap step before the user has written anything else.

## CLI surface

```
cook pull <name>...           pull one or more named modules
cook pull --all               pull every module in the registry
cook pull --list              print available modules; no writes
cook pull --force             treat all overwrite prompts as "yes"
cook pull --accept-trust      non-interactive trust consent (CI)
cook pull --non-interactive   error on any prompt instead of asking
cook pull --registry <url>    one-shot URL override
```

Mutual exclusion:

- `--list` is incompatible with positional names and with `--all`.
- `--all` is incompatible with positional names.
- `--force` does **not** bypass trust; trust has its own flag (`--accept-trust`).

## Registry layout

A registry is a public git repository whose root contains a `modules/` directory. Each immediate subdirectory of `modules/` is one module; its name is the directory name.

```
<registry-repo>/
├── README.md              ← registry-level docs; ignored by `cook pull`
├── LICENSE                ← ignored
└── modules/
    ├── cpp/
    │   ├── init.lua
    │   └── helpers.lua
    ├── rust/
    │   └── init.lua
    └── pnpm-monorepo/
        └── init.lua
```

The registry repo is a normal git repo and may have whatever top-level files it likes (CI configs, READMEs, contribution guidelines, license). `cook pull` only reads under `modules/`. Top-level files are ignored, which is why `modules/` is a required prefix rather than allowing modules at repo root.

A module is always a directory, even if it contains a single `init.lua`. This matches the existing `cook_modules/<name>/init.lua` resolution path supported by the loader (`cook-luaotp/src/pool.rs:430`) and gives module authors room to ship supporting files (helpers, fixtures, READMEs) without restructuring later.

## Configuration

User-level config lives under the platform-idiomatic config dir, resolved via the `dirs` crate:

| OS      | Path                                              |
|---------|---------------------------------------------------|
| Linux   | `$XDG_CONFIG_HOME/cook/`, default `~/.config/cook/` |
| macOS   | `~/Library/Application Support/cook/`             |
| Windows | `%APPDATA%\cook\`                                 |

Two files in this directory:

- **`cook.toml`** — user config. v1 shape:
  ```toml
  [registry]
  url = "https://gilberthouse.story-pike.ts.net/cook/registry"
  ```
- **`trust.toml`** — TOFU consent log (see §Trust model).

Registry URL resolution order (first wins):

1. `--registry <url>` flag
2. `COOK_REGISTRY_URL` environment variable
3. `[registry].url` from `cook.toml`
4. Built-in default: the public Cook registry hosted by Liora Labs.

The built-in default URL is a compile-time constant so a fresh install has a working `cook pull` with no configuration. The exact public URL is finalized at v1 release; the example URLs throughout this document (`https://gilberthouse.story-pike.ts.net/cook/registry`) are illustrative placeholders, not the canonical value.

## Trust model

The first time `cook pull` is invoked against a given registry URL, it prints a disclaimer and prompts for consent. Consent is recorded per-URL in `trust.toml` and is not asked again for that URL.

```
The registry at <URL> contains Lua modules that `cook` will execute when
your recipes use them. By continuing, you trust this registry and the people
who publish to it.

Cook will record your consent in <trust-file-path> so you won't see this
prompt again for this URL.

Trust this registry? [y/N]:
```

`trust.toml` shape:

```toml
[[trusted]]
url = "https://gilberthouse.story-pike.ts.net/cook/registry"
accepted_at = "2026-05-07T14:22:00Z"

[[trusted]]
url = "https://github.com/example/cook-modules"
accepted_at = "2026-06-12T09:01:33Z"
```

Behavior:

- Trust is **per-URL**, not global. Switching to a different registry URL — including legitimate cases like a fork or a corporate mirror — re-triggers the prompt. Changing where executable code comes from is exactly the moment to ask again.
- A user who edits `trust.toml` to remove a line will be re-prompted next pull. The file is documented as user-editable; tampering is the user's prerogative.
- `--accept-trust` is the non-interactive consent path. It writes the same record to `trust.toml`. CI / scripted bootstrap should use this flag.
- `--non-interactive` without `--accept-trust`, against an untrusted URL, is an error. There is no `--no-trust-check` escape hatch; the security story is honest by being mandatory.
- If `trust.toml` cannot be written (read-only home, etc.), `cook pull` warns to stderr and proceeds with consent for this invocation only. The user will be re-prompted next time.
- A corrupt `trust.toml` (unparseable) is treated as empty and the user is re-prompted. The original file is left untouched; the new entry is appended on success.

## Architecture

New code lives under `cli/crates/cook-cli/src/pull/`. No new workspace crate: the surface is small and the only crate-level concern (HTTP) is already satisfied by `ureq`, which `cook-cache` already depends on.

```
cli/crates/cook-cli/src/pull/
├── mod.rs        public entry: pub fn run(args: PullArgs) -> ExitCode
├── config.rs     RegistryConfig: URL resolution from flag/env/file/default
├── trust.rs      ensure_trusted(url, accept_flag, trust_file_path) -> Result<()>
├── fetch.rs      fetch_archive(url) -> impl Read; ureq GET on archive endpoint
├── archive.rs    parse_archive(reader) -> ArchivePlan; pure tar/gzip logic
├── install.rs    install_module(plan, name, dest_root, prompter, force) -> Stats
├── prompt.rs     ConflictPrompter trait + StdinPrompter impl + scripted test impl
└── errors.rs     PullError enum + ExitCode mapping
```

Module responsibilities:

- **`mod.rs`** owns the orchestration: parse args, resolve config, ensure trust, fetch once, dispatch on `--list` / `--all` / names, print summary. ~80 LoC of glue.
- **`config.rs`** is pure: takes `(flag_url: Option<&str>, env: &EnvSnapshot, config_path: &Path)`, returns `Url`. Tests inject env + config path.
- **`trust.rs`** takes `trust_file_path: &Path` and a prompter trait, never hardcodes the user's home dir. Tests use `tempdir`.
- **`fetch.rs`** is the only network boundary. It returns `impl Read` so callers stream straight into `archive.rs`. `ureq::Agent` configured with a sensible timeout (15s connect, 60s total) and the standard `User-Agent` (`cook/<version>`).
- **`archive.rs`** consumes a `Read`, decompresses gzip via `flate2::read::GzDecoder`, walks tar entries via `tar::Archive`, groups them by `modules/<name>/...` prefix into an in-memory `ArchivePlan { modules: BTreeMap<String, Vec<ArchiveEntry>> }`. Forge tarballs prefix every entry with a top-level `<repo>-<sha>/` directory; the archive parser strips that prefix transparently. Per project convention (CLAUDE.md), the plan uses `BTreeMap` for deterministic iteration.
- **`install.rs`** takes a parsed plan and writes one module's tree into `dest_root/<name>/`. Per file: check existence, prompt on conflict, write to `<target>.cook-pull-tmp`, fsync, rename. If the prompter says `q`, any temp files for this module are deleted; already-renamed files stay (overwrite has happened) — this is the only partial-state case and is documented.
- **`prompt.rs`** abstracts stdin so tests don't depend on a TTY. The trait surface is `fn prompt(&mut self, path: &Path) -> ConflictAnswer` returning `Yes | No | All | Quit`.
- **`errors.rs`** is the exit-code source of truth.

Dependency budget added to `cook-cli`:

| Crate    | Purpose                       | Notes |
|----------|-------------------------------|-------|
| `ureq`   | HTTP/HTTPS client             | Already in `cook-cache`; reuse same version. |
| `flate2` | gzip decompression            | Pure Rust default features. |
| `tar`    | tar entry iteration           | Standard, lightweight. |
| `dirs`   | platform config dir           | Tiny, no transitive bloat. |
| `toml`   | parse/serialize config files  | Already in workspace via cache config. |

## Data flow

```
cook pull cpp rust
  │
  ├── 1. config::resolve(flag_url, env, config_path) → Url
  │
  ├── 2. trust::ensure_trusted(url, accept_flag, trust_file_path)
  │       on miss: print disclaimer, prompt y/N (or --accept-trust),
  │                append to trust.toml on accept
  │
  ├── 3. fetch::fetch_archive(url) → impl Read
  │       GET <url>/archive/main.tar.gz, 60s deadline,
  │       response body is the streamed Read
  │
  ├── 4. archive::parse_archive(reader) → ArchivePlan
  │       GzDecoder → tar::Archive → walk entries,
  │       strip <repo>-<sha>/ prefix,
  │       group by modules/<name>/, ignore non-modules/ paths
  │
  ├── 5. dispatch:
  │       --list → print plan.modules.keys(), exit 0
  │       --all  → for each name in plan: install
  │       names  → for each name in args:
  │                  if name not in plan: error 3, list available, abort
  │                  else: install
  │
  └── 6. for each install:
         install::install_module(plan, name, "./cook_modules", prompter, force)
            → Stats { written, overwritten, skipped }
         print summary line per module
```

The archive is fetched **once** per command invocation and served from memory for all requested modules. This keeps `cook pull cpp rust pnpm-monorepo` to a single round-trip.

## Error model

| Condition                                         | Stderr message                                                       | Exit |
|---------------------------------------------------|----------------------------------------------------------------------|------|
| Network failure                                   | `failed to fetch <url>: <underlying>`                                | 1    |
| Trust refused or not established (non-interactive)| `registry <url> is not trusted; rerun with --accept-trust`           | 2    |
| Module not found in registry                      | `module '<name>' not found; available: <list>`                       | 3    |
| Conflict, non-TTY, no `--force`                   | `would overwrite: <paths>; rerun with --force or in a TTY`           | 4    |
| User answered `q` at a prompt                     | `aborted by user`                                                    | 5    |
| Invalid CLI args                                  | clap-generated message                                               | 64   |
| Cannot write `trust.toml`                         | warn-only: `cook: cannot persist trust to <path>: <err>; consent applies for this invocation only` | 0 (warn) |

Whole-command abort on first error is the rule. v1 does not attempt to "pull what it can"; partial bootstrap state is more confusing than no state.

## Behavior details

- **Atomic writes.** Each file is written to `<target>.cook-pull-tmp`, fsync'd, then renamed. A crash mid-pull leaves the user with at most a few `.cook-pull-tmp` files alongside completed renames; no half-written `.lua` files.
- **Permissions.** Files in `cook_modules/` are written with mode `0644`; directories with `0755`. Tar entry modes are ignored. (No reason a Lua module needs to be executable; ignoring tar mode also closes the door on a registry shipping `0777`.)
- **Symlinks in tar entries.** Rejected outright. Any `tar::EntryType::Symlink` or `Link` aborts with `archive contains link entry: <path>`. v1 does not need symlinks and admitting them invites path-escape bugs.
- **Path traversal.** `archive.rs` rejects any entry whose path, after the standard prefix strip, contains `..` or is absolute. Belt-and-braces with the symlink rule.
- **Empty modules.** A `modules/<name>/` directory with no files is treated as not present (omitted from `--list`, errors with "not found" if requested). The registry author needs to put at least an `init.lua`.
- **Duplicate names on the command line.** `cook pull cpp cpp` is silently deduped before dispatch. Order of distinct names is preserved for the summary output.

## Testing

Unit tests, all under `cli/crates/cook-cli/src/pull/`:

- **`archive.rs`** — fixture `.tar.gz` files committed under `pull/test_fixtures/`. Cover:
  - Two modules, one nested deeper than the other.
  - Top-level non-`modules/` paths (skipped).
  - Symlink entry → error.
  - Path-traversal entry → error.
  - Empty archive.
  - Forge prefix (`registry-abc123/`) stripped correctly.
- **`prompt.rs`** — trait + `BufRead`-driven test impl. Covers `y`, `n`, `a`, `q`, EOF, invalid input → re-prompt.
- **`trust.rs`** — `tempdir`-backed trust file. Covers: fresh accept, repeat is silent, corrupt file → re-prompt and original preserved, read-only path → graceful warn.
- **`config.rs`** — flag > env > file > default precedence; malformed TOML is an error; missing config file falls through to default.
- **`install.rs`** — exercises atomic write, conflict prompter dispatch, `--force` short-circuit, `q` cleanup of in-flight temps.

One end-to-end integration test using `mockito`:

- `mockito` serves a fixture tarball at `/archive/main.tar.gz`.
- `cook_cli::pull::run` is invoked with a tempdir as cwd, a tempdir trust file, and `--accept-trust`.
- Asserts: files written under `cook_modules/<name>/`, contents match fixture, `trust.toml` contains the mockito URL, exit code 0.

No live network in any test. No reliance on the user's real config dir.

## Documentation

Out of scope for the implementation plan but listed here so it's not lost:

- README.md gets a short "Pulling community modules" section pointing at `cook pull --list` and noting that pulled modules are committed to the user's project, not tracked centrally.
- The registry repo (separate repo, not in `cli/`) gets its own README documenting the `modules/` layout convention so external contributors know where to put things if/when the trust model permits external publishers.
- A user-facing note explaining that re-pulling is the only update path.

## Future work (deliberately out of scope for v1)

These are noted for context, not promised:

- **Integrity layer.** When/if external publishers want to add modules to the canonical registry, the trust model needs a content-integrity story — at minimum a manifest committed and signed, ideally per-module checksums baked into a `cook pull --verify` flag. The `trust.toml` surface is forward-compatible: a future record can carry a `pinned_commit_sha` field that cooks pull verifies against.
- **Private registries.** A `[registry.auth]` block in `cook.toml` carrying a token from an env var, mapped to an `Authorization: Bearer` header. Mechanically tiny; the reason to defer is that v1 has no production private registry to test against.
- **Update awareness.** Storing the registry's commit SHA at pull time in a sidecar would let a future `cook pull --check` tell users "your `cpp` module is N commits behind upstream" without re-installing. v1 has no such sidecar; adding one later is non-breaking.
- **Multi-registry composition.** Named registries in `cook.toml` (`cook pull official:cpp`, `cook pull mycorp:internal-thing`). Designed against in v1 because the YAGNI cost of a single string identifier is higher than the YAGNI cost of one URL.
