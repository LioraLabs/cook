local M = {}

local INSTALL_HINT = "apt: libraylib-dev / brew: raylib"

local MAC_FRAMEWORKS = { "OpenGL", "Cocoa", "IOKit", "CoreVideo", "CoreAudio" }

local function ensure_mac_frameworks(payload)
    if cook.platform.os ~= "macos" then return end
    local present = {}
    for _, fw in ipairs(payload.frameworks) do present[fw] = true end
    for _, fw in ipairs(MAC_FRAMEWORKS) do
        if not present[fw] then payload.frameworks[#payload.frameworks + 1] = fw end
    end
end

function M.find(opts)
    opts = opts or {}
    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("raylib")
    if a then
        ensure_mac_frameworks(a.payload)
        if opts.version then
            local ver = require("cook_cc.version")
            if not (a.payload.version and ver.satisfies(a.payload.version, opts.version)) then
                return { strategy = "curated:raylib", outcome = "miss",
                         reason = "pkg-config version " .. (a.payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return a
    end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("raylib")
    if b then
        ensure_mac_frameworks(b.payload)
        local header = require("cook_cc.finders.header_probe")
        local v = header.parse_define("/usr/include/raylib.h", "RAYLIB_VERSION")
        if v then b.payload.version = v end
        if opts.version then
            local ver = require("cook_cc.version")
            if not (b.payload.version and ver.satisfies(b.payload.version, opts.version)) then
                return { strategy = "curated:raylib", outcome = "miss",
                         reason = "bare-probe version " .. (b.payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return b
    end

    return { strategy = "curated:raylib", outcome = "miss",
             reason = "neither pkg-config 'raylib' nor libraylib on default linker paths",
             hint = INSTALL_HINT }
end

return M
