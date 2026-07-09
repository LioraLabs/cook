Pins CS-0134's removal of the `>>` register-phase sigil: a `>> foo.bar()`
line inside a `recipe` body is now a parse error. Register-phase module
calls under a recipe body no longer need (or accept) a sigil — write the
bare `module.call()` line instead; it auto-classifies as register-phase Lua.
