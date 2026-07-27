# cook-probe

`cook-probe` atomically materializes shared probe values.

## How it does that well

- It depends inward on `cook-contracts` for canonical probe value meaning.
- It provides shared materialization mechanics for registration and execution.
- It publishes probe values with same-directory atomic replacement, preserving
  either the old complete value or the new complete value.

It does not define contracts, choose a VM or sandbox, schedule work, or own
cache policy. Those decisions remain with the runtime adapters.
