local M = {}

local function search_dirs()
    local cached = cook.cache.get("cc.linker-search-dirs")
    if cached then return cached end
    local dirs = { "/usr/lib", "/usr/local/lib" }
    local ok, out = pcall(cook.sh, "cc -print-search-dirs")
    if ok and out then
        local libs_line = out:match("libraries:%s*=([^\n]+)")
        if libs_line then
            for d in libs_line:gmatch("[^:]+") do dirs[#dirs + 1] = d end
        end
    end
    cook.cache.set("cc.linker-search-dirs", dirs)
    return dirs
end

local function extensions()
    if cook.platform.os == "macos" then
        return { ".dylib", ".so", ".a" }
    end
    return { ".so", ".dylib", ".a" }
end

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

-- CS-0045 forbids `fs.exists(...)` on paths outside the project sandbox;
-- the bare probe by design walks system linker directories like /usr/lib
-- which are always outside. Existence checks therefore go through
-- `cook.sh`, which is unsandboxed and inherently shell-out. Single-quote
-- the path and escape any embedded `'` as `'\''` so a hostile lib name
-- cannot inject shell metacharacters. (Callers feed cc.find names that
-- are also passed to `-l<name>`; the linker already constrains those to
-- a tame charset, but defense in depth is free here.)
local function exists_unsandboxed(path)
    local quoted = "'" .. (path:gsub("'", "'\\''")) .. "'"
    local ok, out = pcall(cook.sh, "test -e " .. quoted .. " && echo y || echo n")
    return ok and (out or ""):match("^y") ~= nil
end

function M.try(name)
    for _, dir in ipairs(search_dirs()) do
        for _, ext in ipairs(extensions()) do
            local p = dir .. "/lib" .. name .. ext
            if exists_unsandboxed(p) then
                local payload = blank_payload()
                payload.system_libs = { name }
                payload.libs = "-l" .. name
                return { strategy = "bare-probe", outcome = "hit", reason = "", payload = payload }
            end
        end
    end
    return nil
end

function M.main_chain(name, opts)
    if opts.version then
        return { strategy = "bare-probe", outcome = "skip",
                 reason = "bare probe cannot verify version constraints" }
    end
    local attempt = M.try(name)
    if attempt then return attempt end
    return { strategy = "bare-probe", outcome = "miss",
             reason = "no lib" .. name .. ".{so,dylib,a} on default linker search paths" }
end

return M
