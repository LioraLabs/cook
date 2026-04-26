Demonstrates the canonical replacement for the removed top-level `variable_declaration` form (CS-0011). Variables are written into `cook.env.X` from inside an unnamed (base) `config` block, exercising §3.6.1 (config-block composition) and §6 (Cook Lua API).

Replaces the now-rejected pattern that the negative case `007-bare-vardecl-rejected` covers.
