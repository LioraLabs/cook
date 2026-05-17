Exercises the new `needs = {...}` field on cc.bin. Per Standard §28.3.14,
each name in `needs` MUST be resolved via a `cc:find:<name>` probe. This
fixture's parse output verifies the recipe-body shape; the register/exec
behavior (probe registration + sigil weaving in compile/link commands)
is covered by the unit tests in `cook_cc/spec/needs_field_spec.lua`.
