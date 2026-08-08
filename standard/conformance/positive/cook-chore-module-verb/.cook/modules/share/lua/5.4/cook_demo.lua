local M = {}

-- Standard §22.11 / CS-0176. Registered at module TOP LEVEL, not from a
-- function the Cookfile calls later: the namespace prefix is checked against
-- the module currently being evaluated, and that identity is exact only while
-- this chunk runs. The consequence pinned by this fixture is that `use
-- cook_demo` alone is what makes the verb invocable — the Cookfile calls
-- nothing.
--
-- `demo.` is the module name with its leading `cook_` stripped, which §22.11
-- admits alongside the full `cook_demo.` spelling.
cook.chore("demo.greet", {origin = "cook_demo.greet"}, function()
    cook.add_unit({command = "echo hello", cache = false})
end)

return M
