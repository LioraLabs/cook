local M = {}

-- Module-local state accumulated across recipe bodies. Nothing here is
-- exported through cook.export/cook.import; the finalizer callbacks below
-- read and write it directly, exercising the "module-held per-VM state
-- MUST still be live when a callback runs" clause of Standard SS22.9.
local marks = {}
local order_flag_set = false

-- mark(name) is called as a bare module_call from each recipe body
-- (register-phase, CS-0134 rule 6). It just records that this body ran.
function M.mark(name)
    marks[name] = true
end

-- arm() queues the two callbacks the fixture pins. It is itself called from
-- a top-level `register` block, so it runs before either recipe body has a
-- chance to register -- proving the callbacks cannot rely on call-site
-- ordering, only on cook.on_register_complete's own after-every-body
-- guarantee.
function M.arm()
    -- First queued callback: proves "after every recipe body has
    -- registered". If cook.on_register_complete ran this before both
    -- `alpha` and `beta`'s bodies had executed their own m.mark(...) call,
    -- one of these marks would still be missing and the case would fail
    -- with a distinctive error naming which one -- there is no other way
    -- for this assertion to pass.
    cook.on_register_complete(function()
        if not marks["alpha"] then
            error("on-register-complete-order: mark 'alpha' missing -- finalizer ran before recipe alpha's body registered")
        end
        if not marks["beta"] then
            error("on-register-complete-order: mark 'beta' missing -- finalizer ran before recipe beta's body registered")
        end
        order_flag_set = true
    end)

    -- Second queued callback: proves registration order. It can only see
    -- order_flag_set = true if the first callback above already ran to
    -- completion -- callbacks run in the order they were queued, exactly
    -- once each.
    cook.on_register_complete(function()
        if not order_flag_set then
            error("on-register-complete-order: order_flag_set not set -- the first queued callback did not run before the second")
        end
    end)
end

return M
