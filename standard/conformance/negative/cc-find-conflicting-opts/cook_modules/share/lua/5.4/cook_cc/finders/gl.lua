local M = {}

local INSTALL_HINT = "apt: libgl-dev / macOS system framework"

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

function M.find(opts)
    opts = opts or {}
    if cook.platform.os == "macos" then
        if opts.version then
            return { strategy = "curated:gl", outcome = "skip",
                     reason = "macOS OpenGL framework has no detectable version" }
        end
        local payload = blank_payload()
        payload.frameworks = { "OpenGL" }
        return { strategy = "curated:gl", outcome = "hit", reason = "", payload = payload }
    end

    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("gl")
    if a then return a end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("GL")
    if b then return b end

    return { strategy = "curated:gl", outcome = "miss",
             reason = "neither pkg-config 'gl' nor libGL on default linker paths",
             hint = INSTALL_HINT }
end

return M
