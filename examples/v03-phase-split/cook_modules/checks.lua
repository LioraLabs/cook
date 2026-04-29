-- Tiny module to demonstrate that module_call lines desugar to register-phase
-- inline_lua per §4.11. The module's functions are called during recipe
-- registration; they typically record work via cook.add_unit, but this one
-- just prints from register so the execution-phase contrast is obvious.

local checks = {}

function checks.greet(who)
    print("[register, via module_call] hello,", who)
end

return checks
