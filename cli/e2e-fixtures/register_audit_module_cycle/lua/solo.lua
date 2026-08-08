-- Module 'solo' loads itself: a degenerate self-cycle. CS-0035 surfaces this
-- as `module cycle detected: lua/solo.lua -> lua/solo.lua`.
local m = {}
cook.load_module("./lua/solo.lua")
return m
