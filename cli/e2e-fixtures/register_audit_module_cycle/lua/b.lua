-- Module 'b' loads module 'a'. See lua/a.lua for the cycle story.
local m = {}
cook.load_module("./lua/a.lua")
return m
