local M = {}

local CURATED = {
    raylib  = "cook_cc.finders.raylib",
    sdl2    = "cook_cc.finders.sdl2",
    openal  = "cook_cc.finders.openal",
    gl      = "cook_cc.finders.gl",
    threads = "cook_cc.finders.threads",
    zlib    = "cook_cc.finders.zlib",
    libcurl = "cook_cc.finders.libcurl",
}

local ALIASES = {
    opengl = "gl",
}

function M.lookup(name)
    local canonical = ALIASES[name] or name
    local mod_path = CURATED[canonical]
    if not mod_path then return nil end
    local ok, mod = pcall(require, mod_path)
    if not ok then return nil end
    return function(opts)
        local attempt = mod.find(opts)
        attempt.strategy = "curated:" .. canonical
        return attempt
    end
end

return M
