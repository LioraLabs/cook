Pins Standard §{cat.probes.member-source} ("Register-phase reads") and
§{lua.add-unit-after} / CS-0219: the two halves of "a module scans, then draws
the graph the scan describes".

`scan:mods` is not a fan-out source. No recipe declares `ingredients
scan:mods`, so the §22.5.10 pre-pass has no reason to evaluate it and does not.
The read in `cook_modgraph.compile_all` is therefore step 2 of §21's
register-phase lookup and nothing else: declared probe, resolved at the moment
of the call. Before CS-0219 the same read returned `nil` — silently, which is
what made the gap expensive to find.

The `after` entries are the other half. Three units, all siblings in one step
group, so they share an entry barrier and would otherwise all run at once; the
edges narrow that to exactly what the import data requires. They name declared
output paths rather than unit names because a unit has no name, and CS-0169
already keeps one output path to one unit.

The assertions live in the Lua because no parse dump can see any of this. A
parse dump sees a `use`, a probe, and a module call; the graph the module draws
exists only after registration. Each assertion errors with `error(msg, 0)`
naming the property that broke, so a failure says which one — the register
harness asserts only that registration returned `Ok`.

The probe emits its modules in dependency order deliberately. An `after` entry
may only name an earlier unit, so the emission order IS the topological order,
and the party that can produce one meaningfully is the module holding the
scanned graph. See `negative/after-unit-registered-later` for what happens when
it does not.
