-- cook_smoke: Phase 3 acceptance fixture rock for SHI-176.
-- Throwaway: published to rocks.usecook.com to verify the install
-- pipeline end-to-end. Not a stable API; do not depend on this from
-- production Cookfiles.

local M = {}

function M.value()
    return 42
end

return M
