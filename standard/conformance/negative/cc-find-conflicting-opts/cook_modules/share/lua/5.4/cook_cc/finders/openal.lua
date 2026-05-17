local M = {}

local INSTALL_HINT = "apt: libopenal-dev / macOS system framework / brew: openal-soft"

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
            return { strategy = "curated:openal", outcome = "skip",
                     reason = "macOS OpenAL framework has no detectable version" }
        end
        local payload = blank_payload()
        payload.frameworks = { "OpenAL" }
        return { strategy = "curated:openal", outcome = "hit", reason = "", payload = payload }
    end

    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("openal")
    if a then return a end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("openal")
    if b then return b end

    return { strategy = "curated:openal", outcome = "miss",
             reason = "neither pkg-config 'openal' nor libopenal on default linker paths",
             hint = INSTALL_HINT }
end

return M
