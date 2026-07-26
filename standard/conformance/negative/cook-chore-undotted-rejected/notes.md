Pins Standard §{lua.cook-chore} / CS-0176: a module-registered chore MUST carry
a dotted prefix identifying the module, and an undotted name is refused at
register phase.

This is the case the whole namespace rule exists for. §{mods.authoring.minting}
scopes its name-ownership MUST to recipes so that `cook.chore` can exist at all,
and the argument for that carve-out — a chore is a command the author invokes by
name, not a DAG node minted behind their back — holds only while undotted names
remain exclusively the author's. `greet` here would collide with an author who
wrote `chore greet` in a Cookfile that merely used this module. Accepting it
would leave §12.7.8 asserted rather than enforced.

The positive half is `positive/cook-chore-module-verb/`.
