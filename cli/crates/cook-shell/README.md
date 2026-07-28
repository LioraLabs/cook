# cook-shell

`cook-shell` runs a shell command and reports what it did.

## How it does that well

- It returns one [`Outcome`] describing the whole event: the bytes the command
  wrote, tagged with the stream each came from, its exit status, and how long
  it took. Callers that need only one of those take one of those.
- It captures **both** streams, always. The bug that motivated this crate is a
  caller that returned stdout and dropped stderr, so a command that succeeded
  with warnings reported none of them.
- It builds a `CommandFailure` in one place. Four call sites used to build one
  each, which is how the formatting fix that opened the code-path unification
  milestone reached one twin and not the other.
- It distinguishes a command that ran and failed (an unsuccessful `Outcome`)
  from one that never started (a `SpawnError`). Callers report those
  differently, and previously some could not tell them apart.
- It states its ordering guarantee and does not exceed it. One spawn yields at
  most one chunk per stream, because `Command::output()` buffers the two
  separately and their true interleaving is not recoverable. A *sequence* of
  spawns preserves its order; a single spawn's two streams do not interleave.
  Cook Standard §{lua.cook-sh} makes that limit normative (CS-0188).

## What it does not do

It does not decide what a command is: callers pass text they have already
resolved, with sigils substituted and shell blocks joined. It does not own the
caller's caches, so disarming `cook-fingerprint`'s stat memo stays with the
execute-phase callers that need it, which also keeps this crate from depending
on `cook-fingerprint`. It does not print, emit events, or drive progress: it
returns an `Outcome` and the caller decides what to say.

It has no timeout. There was one on the test path and it never fired, because
CS-0135 removed the modifier that set it; the unreachable kill loop was the only
reason that path drained its pipes by hand instead of calling `output()`. A
timeout belongs here the day a caller can set one.

## Relationship to `cook-contracts`

`cook-contracts` owns what the values MEAN: `OutputChunk`, `OutputStream`,
`CommandFailure`, `CapturedStream`. It is forbidden stateful standard-library
access by its own layout test, so it can describe a command's result but never
produce one. This crate produces one. Nothing depends back on it.
