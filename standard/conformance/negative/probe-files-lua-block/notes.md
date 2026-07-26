Pins §22.5.2 (CS-0148): `files >{ … }` is rejected — the brace content of the
`files` kind is a GLOB LIST, not a body. The `files` twin of
`probe-as-tools-lua-block` / `probe-as-env-lua-block`. A Lua body would have
to return the manifest itself, defeating the `@files-manifest` lowering whose
whole point is that the value and the fingerprint's FILES section are computed
from the same bytes.
