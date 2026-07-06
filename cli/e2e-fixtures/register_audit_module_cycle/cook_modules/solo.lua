-- Module 'solo' loads itself: a degenerate self-cycle. CS-0035 surfaces this
-- as `module cycle detected: solo -> solo`.
local m = {}
cook.load_module("solo")
return m
