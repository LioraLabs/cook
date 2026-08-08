local M = {}

-- Parses `#define MACRO "X.Y.Z"` or `#define MACRO X.Y.Z` from a header file.
-- Returns the captured value as a string, or nil if not found / unreadable.
function M.parse_define(path, macro)
    if not fs.exists(path) then return nil end
    local content = fs.read(path) or ""
    local v = content:match("#define%s+" .. macro .. "%s+\"([^\"]+)\"")
    if v then return v end
    v = content:match("#define%s+" .. macro .. "%s+([%w%.%-]+)")
    return v
end

return M
