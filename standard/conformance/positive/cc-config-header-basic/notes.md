Exercises §28.3.15 cc.config_header: walks the vars table, collects
the cc.checks.has_header sigil into the unit's probes list, emits a
cook.add_unit whose command invokes the vendored renderer. The
parse fixture captures the recipe-level shape; runtime substitution
behaviour is covered by the busted tests in cook_cc/spec/.
