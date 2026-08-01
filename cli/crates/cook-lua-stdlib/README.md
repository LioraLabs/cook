# cook-lua-stdlib

`cook-lua-stdlib` is the one implementation of everything Cook's two Lua VMs do
identically: the register-phase VM (`cook-register`) and the execute-phase
worker VMs (`cook-luaotp`) install the same surface from here, so behaviour the
Standard marks **Phase: Both** cannot be taught to one phase and not the other.

## How it does that well

- It abstracts the *only* real difference between the two callers instead of
  forking on it. `cook-register` knows its working directory at VM creation and
  never changes it; a `cook-luaotp` worker is reused across items from different
  Cookfiles (CS-0017 imports), so its cwd moves per item. [`WorkingDirSource`]
  is `Static` or `Live`, `Live` resolves on every call, and one `fs_api` serves
  both. [`SandboxSource`] mirrors the split so the policy is per work item too.
- It holds THE Lua↔JSON codec (`json_codec.rs`, CS-0198/COOK-388). Both phases
  serialize probe values into the same store and the same `seal_contribution`
  fingerprint, so they must agree byte-for-byte; they used to be
  manually-synchronized twins in `cook_register::probe_value` and
  `cook_luaotp::probe_value`, plus a third weaker walker on the module-export
  path that turned a number outside i64/f64 range into `0.0` where the twins
  raised. The twins are now re-export shims and the agreement test runs two
  independently-created VMs to identical canonical bytes.
- The codec refuses rather than coerces, and says where. Non-UTF-8 strings,
  non-finite numbers, cycles, mixed string/integer keys, and array holes are all
  errors carrying the offending path (`.deps[3].name`), because CS-0102's value
  contract is what the store and the hash are defined over, and a value that
  quietly changed shape on the way in is a cache key that quietly means
  something else.
- `fs.glob` gates the pattern and every match. Sandboxing only the pattern lets
  `fs.glob("../*")` cross the root mid-pattern, so each result is re-checked
  (CS-0045). Directory matches are dropped (CS-0064) because the one downstream
  consumer, `cook.add_unit` inputs, rejects directory paths (CS-0063), and
  raising that diagnostic for a path the author never typed is a bad trade.
- `fs.write` and `fs.remove` disarm `cook-fingerprint`'s stat memo (COOK-306). A
  Lua-side write is a write to the working tree like any other; a memoised mtime
  that survived it would hand the next fingerprint a stale answer, which is the
  failure mode that does not look like a bug.
- The sandbox normalizes lexically, not via `fs::canonicalize`, and says what
  that costs. `fs.write` and `fs.mkdir_p` must succeed against paths that do not
  yet exist, so the prefix check is component-wise over an absolutized, lexically
  normalized candidate; CS-0045 does not attempt to defeat hostile symlinks
  already planted in the project, and the module doc states so rather than
  implying a guarantee it does not provide.
- The shell escape hatches are shimmed, not deleted. `os.execute` and `io.popen`
  consult the live policy per call, and the refusal names the fix: use `cook.sh`
  (working-directory rooted and recorded in the unit's `command_hash`) or move
  the work to a `chore`. Deleting them would have produced a nil-index error
  naming nothing.
- `cook.tools.id` is backed by `cook-fingerprint`'s per-run tool-hash memo, so a
  module can fold a toolchain's identity into a sealed value without hashing a
  60MB binary from Lua, and the returned table separates `hash` (foldable) from
  `path` (machine-specific, MUST NOT be sealed, §12.7.5, CS-0158/COOK-277).

## What it does not do

It does not create or own a Lua VM. Every entry point takes a `&Lua` (and, where
the surface hangs off `cook`, the `cook` table itself) so the phase crate keeps
control of construction, `package.path`, module loading, and globals layout.

It does not host phase-specific surfaces. `cook.sh`, `cook.probes`,
`cook.export`/`cook.import`, `cook.add_unit`, and the registration verbs differ
in mechanism between phases (a register-phase pre-pass store versus a worker's
`SharedProbeValueStore`), so they stay in `cook-register` and `cook-luaotp`.
The line is mechanism, not spelling: when only the spelling differs, it belongs
here.

It does not own the canonical JSON encoding. `encode_canonical_json` and
`decode_json` are pure and live in `cook_contracts::probe_value`; this crate
walks Lua values to and from `serde_json::Value` and stops there. `cook-contracts`
is a dev-dependency only, so the codec suite can pin store-byte round trips
without making the edge production.

It does not hash. `cook.tools.id` and the stat-memo disarm are both calls into
`cook-fingerprint`.

Two things here are honest exceptions rather than design:

- `cook.cookfile.*` is register-phase only (Standard §22.13) and is the sole
  reason for the `cook-cookfile` dependency. It lives here because what it
  reuses is the *sandbox gate*, not the phase: `check_path`, `WorkingDirSource`,
  and `SandboxSource`. That is a defensible reason and it is still the one entry
  that would not be re-derived from this crate's charter.
- `register_fs_api` (the no-sandbox wrapper) has no production caller. Since
  CS-0135 retired `plate`, no step kind selects `SandboxPolicy::Off`; it survives
  as the worker's initial slot value and in this crate's tests. A permissive
  constructor with no caller is a default waiting to be picked up by accident.

## Relationship to `cook-contracts`

`cook-contracts` is the home for law that is pure: plain data and functions of
their arguments, budget serde only. The moment a law needs `mlua`, it cannot go
there, and the stratum rule sends it here instead. That is exactly the Lua↔JSON
walkers' story: the old copies were justified against `cook-contracts` alone,
which was correct and stayed correct, and the justification silently expired the
day CS-0123 made this crate a shared dependency of both phases. Nobody could
tell, because the comments named only the home that was still refusing.

The dividing question for anything new: does answering it require touching
`mlua`, the filesystem, or a process? If not, it belongs upstream in
`cook-contracts`. If it needs `mlua` and both phases must agree on the answer,
it belongs here, and it belongs here *once*.
