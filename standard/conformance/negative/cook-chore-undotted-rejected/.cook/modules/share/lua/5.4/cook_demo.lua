local M = {}

-- Standard §22.11 / CS-0176: an undotted chore name is REJECTED. `greet` would
-- put a module-registered name into the space of undotted names that belong to
-- the Cookfile author (§12.7.8) — precisely the collision the namespace rule
-- exists to make impossible.
cook.chore("greet", {}, function()
    cook.add_unit({command = "echo hello", cache = false})
end)

return M
