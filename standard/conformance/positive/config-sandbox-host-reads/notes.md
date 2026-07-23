# config-sandbox-host-reads

The `host.*` read surface is the only channel for external input into a
sandboxed `config` body (Standard §5.3.2, CS-0163). This fixture exercises all
four accessors in one base config block:

- `host.os` / `host.arch` — host facts (replace `cook.platform.*` inside config)
- `host.env(name, default)` — ambient env read with fallback (replaces
  `os.getenv(name) or default`)
- `host.read(path)` — file contents as a string (replaces `io.open(path)`),
  resolved relative to the Cookfile's directory; `version.txt` is the sibling

`register_ok.txt` marks this for the `cook-register/tests/conformance.rs`
positive harness, which drives config dispatch and asserts the block registers
cleanly. `parse.txt` is the AST-shape golden for the parser harness (the config
body is opaque `LUA_SOURCE`, so it round-trips verbatim).

Each `host.*` read is recorded as a determinant for provenance
(`RegisteredCookfile.config_host_reads`); it is not a second cache-key channel —
host-varying config values re-key consuming steps through consulted-value
hashing (milestone §E).
