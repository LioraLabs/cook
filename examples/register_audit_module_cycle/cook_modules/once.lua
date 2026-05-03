-- Module 'once' increments a global counter from its top-level body and from
-- init(). CS-0035 memoizes successful loads, so repeated cook.load_module("once")
-- on the same VM must not re-evaluate the file or re-run init().
local m = {}
_G.once_top_level_calls = (_G.once_top_level_calls or 0) + 1
function m.init()
    _G.once_init_calls = (_G.once_init_calls or 0) + 1
end
return m
