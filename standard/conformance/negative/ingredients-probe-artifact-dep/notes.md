§22.5.9 static-input rule. Probe `data`'s file input `gen.json` is the declared
output of recipe `gen` — a build artifact. An `ingredients <probe>` source is
resolved by the register pre-pass before any recipe runs, so depending on a
not-yet-built artifact is incoherent and rejected. (gen.json is committed so the
pre-pass produce succeeds; the rejection is on the artifact dependency, not a
read error.)
