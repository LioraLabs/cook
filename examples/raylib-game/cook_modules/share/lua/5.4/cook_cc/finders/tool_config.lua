local M = {}

-- Runs `<tool> <args>` via cook.sh; returns trimmed stdout on success, nil on error.
function M.try(tool, args)
    local cmd = tool .. " " .. args
    local ok, out = pcall(cook.sh, cmd)
    if not ok then return nil end
    return (out or ""):gsub("%s+$", "")
end

return M
