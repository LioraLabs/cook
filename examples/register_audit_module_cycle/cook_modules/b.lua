-- Module 'b' loads module 'a'. See cook_modules/a.lua for the cycle story.
local m = {}
cook.load_module("a")
return m
