-- Standard §7.6 / CS-0218: a chore body runs only when the chore is the
-- dispatch target or is reachable from it.
--
-- The observable has to be a REGISTER-time effect, which is why the chore is
-- registered through `cook.chore` (§22.12) rather than written as a surface
-- `chore` block. A surface chore body admits no register-phase step at all
-- (§7.2, CS-0134): every line in it becomes a work unit, so a `>` step's
-- `error(...)` fires at execute phase, when the unit runs — which for a chore
-- nobody invoked is never. Such a fixture passes whatever the register phase
-- does, and an earlier draft of this one did exactly that.
--
-- A `cook.chore` body is a plain Lua function called during the register pass,
-- so raising in it is observable precisely when the pass invokes it.

local M = {}

-- A no-op target maker, so the recipe body is a real module call. It is not
-- what is under test; it is here so the fixture looks like the shape this
-- rule protects — a module offering both build makers and project verbs.
function M.bin(_) end

-- The verb. Paramless, which before CS-0218 was the property that got a body
-- invoked on every register pass regardless of the target: `__params` was
-- standing in for "safe to run unasked", and it never meant that.
--
-- `error` rather than a marker file: a `register_ok` fixture's one channel is
-- whether the pass completed, and a fixture that wrote anything would not be
-- idempotent over a corpus that is run repeatedly.
cook.chore("verb.scaffold", {}, function()
    error("CS-0218: this chore body must not be invoked speculatively", 0)
end)

-- The parametric sibling, whose skip predates this amendment (Note 7.5.1.1).
-- It is here so the fixture pins the RULE rather than one arm of it: both
-- chores are speculative in this pass and neither body may run.
cook.chore("verb.deploy",
    { __params = { { kind = "required", name = "target" } } },
    function(_)
        error("CS-0218: a parametric chore body must not be invoked either", 0)
    end)

return M
