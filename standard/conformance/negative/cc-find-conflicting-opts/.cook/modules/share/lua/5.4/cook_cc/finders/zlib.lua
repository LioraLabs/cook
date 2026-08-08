local M = {}

local INSTALL_HINT = "apt: zlib1g-dev / macOS system / brew: zlib"

local function check_version(payload, constraint)
    if not constraint then return true end
    if not payload.version then return false end
    local ver = require("cook_cc.version")
    return ver.satisfies(payload.version, constraint)
end

function M.find(opts)
    opts = opts or {}
    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("zlib")
    if a and check_version(a.payload, opts.version) then return a end
    if a and not check_version(a.payload, opts.version) then
        return { strategy = "curated:zlib", outcome = "miss",
                 reason = "pkg-config version " .. (a.payload.version or "(unknown)")
                          .. " does not satisfy " .. opts.version,
                 hint = INSTALL_HINT }
    end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("z")
    if b then
        local header = require("cook_cc.finders.header_probe")
        local v = header.parse_define("/usr/include/zlib.h", "ZLIB_VERSION")
        if v then b.payload.version = v end
        if check_version(b.payload, opts.version) then return b end
        return { strategy = "curated:zlib", outcome = "miss",
                 reason = "bare-probe version " .. (b.payload.version or "(undetectable)")
                          .. " does not satisfy " .. opts.version,
                 hint = INSTALL_HINT }
    end

    return { strategy = "curated:zlib", outcome = "miss",
             reason = "neither pkg-config nor bare probe located zlib",
             hint = INSTALL_HINT }
end

return M
