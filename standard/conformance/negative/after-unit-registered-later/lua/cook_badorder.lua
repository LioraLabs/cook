-- Standard §{lua.add-unit-after} / CS-0219: an `after` entry may only name an
-- output declared by an EARLIER unit of the same recipe. Here the consumer is
-- registered first, so `build/foo.bmi` names a unit that does not exist yet.
local M = {}

function M.emit()
    cook.step_group(function()
        cook.add_unit({
            outputs = { "build/bar.o" },
            command = "compile bar",
            after   = { "build/foo.bmi" },
        })
        cook.add_unit({
            outputs = { "build/foo.bmi" },
            command = "compile foo",
        })
    end)
end

return M
