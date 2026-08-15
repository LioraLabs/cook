Pins the top-level `files` declaration: its body is a glob list, not Lua. A Lua body would have
to return the manifest itself, defeating the `@files-manifest` lowering whose
whole point is that the value and the fingerprint's FILES section are computed
from the same bytes.
