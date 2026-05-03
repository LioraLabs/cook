# cache_audit_first_token

Walkthrough fixture for the CS-0035 bug fix to `cook-cache::context::step_context_hash`.

Pre-CS-0035, the per-step context hash fingerprinted only the FIRST
whitespace-delimited token of the command. For any multi-line shell script
(the dominant Cookfile pattern) only the first line's first token was
hashed; tool binaries invoked on later lines (`gcc`, the linker, etc.)
were silently ignored. Swapping the toolchain therefore did NOT invalidate
the cache.

This fixture pins the post-fix behavior end-to-end through the cook binary.
See `walkthrough.sh` for the assertions.
