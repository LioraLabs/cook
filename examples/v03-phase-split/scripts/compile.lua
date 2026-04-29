local M = {}
function M.run()
    print("[compile.run] sources:", table.concat(fs.glob("*.lua") or {}, ","))
    cook.sh("echo from-script")
end
return M
