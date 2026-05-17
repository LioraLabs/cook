local M = {}

local PROBE_KEY = "cc:linker-search-dirs"
local probe_registered = false

local function produce_body()
    return [[
        local dirs = { "/usr/lib", "/usr/local/lib" }
        local ok, out = pcall(cook.sh, "cc -print-search-dirs 2>/dev/null")
        if ok and out then
            local libs_line = out:match("libraries:%s*=([^\n]+)")
            if libs_line then
                for d in libs_line:gmatch("[^:]+") do dirs[#dirs + 1] = d end
            end
        end
        return dirs
    ]]
end

local function ensure_probe()
    if probe_registered then return end
    cook.probe(PROBE_KEY, {
        inputs = { tools = { "cc" }, env = { "LIBRARY_PATH" } },
        produce = produce_body(),
    })
    probe_registered = true
end

ensure_probe()

local function search_dirs()
    return cook.cache.get(PROBE_KEY) or { "/usr/lib", "/usr/local/lib" }
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

-- CS-0045: bare probe walks system paths outside the project sandbox.
-- Use cook.sh "test -e" with shell-quote escaping (single-quote, escape
-- embedded ' as '\''). Linker constrains lib names to a tame charset;
-- this is defense in depth.
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
