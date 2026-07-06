-- Module 'a' tries to load module 'b' at load time.
-- Combined with cook_modules/b.lua's reverse load, this forms a 2-node
-- cycle a -> b -> a. Pre-CS-0035 the loader had no in-flight set, so this
-- recursed indefinitely and stack-overflowed. CS-0035 makes the loader
-- raise `module cycle detected: a -> b -> a` instead.
local m = {}
cook.load_module("b")
return m
