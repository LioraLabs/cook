local toolchain     = require("cook_cc.toolchain")
local cc            = require("cook_cc.cc")
local targets       = require("cook_cc.targets")
local finder        = require("cook_cc.finder")
local db            = require("cook_cc.compile_db")
local checks        = require("cook_cc.checks")
local config_header = require("cook_cc.config_header")

local M = {}

function M.init()
    -- Toolchain probe is registered lazily on first get_compiler() / target-maker call.
end

-- Public surface (Standard §28 v0.6 contract).
M.toolchain        = toolchain.set
M.defaults         = toolchain.merge_defaults
M.compile          = cc.compile
M.archive          = cc.archive
M.link             = cc.link
M.bin              = targets.bin
M.lib              = targets.lib
M.shared           = targets.shared
M.headers          = targets.headers
M.find             = finder.find
M.find_or_error    = finder.find_or_error
M.register_finder  = finder.register
M.compile_commands = db.write
M.checks           = checks
M.config_header    = config_header.config_header

return M
