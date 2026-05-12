local M = {}

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

function M.find(opts)
    opts = opts or {}
    if opts.version then
        return { strategy = "curated:threads", outcome = "skip",
                 reason = "threads has no detectable version; constraint cannot be honoured" }
    end
    local payload = blank_payload()
    if cook.platform.os ~= "macos" then
        payload.cflags = "-pthread"
        payload.libs   = "-pthread"
    end
    return { strategy = "curated:threads", outcome = "hit", reason = "", payload = payload }
end

return M
