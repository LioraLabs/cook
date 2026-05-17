local cjson = require("cjson")

local M = {}

local function is_c(source) return source:match("%.[cC]$") ~= nil end

local function shell_chomp(s) return (s or ""):gsub("%s+$", "") end

local function format_command(src, ci)
    local compiler = ci.compiler or "g++"
    if is_c(src) then
        if compiler:match("g%+%+")    then compiler = "gcc"
        elseif compiler:match("clang") then compiler = "clang"
        else compiler = "cc" end
    end
    local flags = { "-c" }
    if ci.standard and not is_c(src) then
        flags[#flags + 1] = "-std=" .. ci.standard
    end
    for _, inc in ipairs(ci.includes or {}) do flags[#flags + 1] = "-I" .. inc end
    for _, def in ipairs(ci.defines  or {}) do flags[#flags + 1] = "-D" .. def end
    local stem = src:match("([^/]+)%.[^.]+$") or src
    -- Trailing space so spec assertions like " -std=... " match the last token.
    return compiler .. " " .. table.concat(flags, " ") .. " " .. src
        .. " -o build/obj/" .. (ci.target_name or "default") .. "/" .. stem .. ".o "
end

function M.write()
    local targets = require("cook_cc.targets")._known()
    if #targets == 0 then
        fs.write("compile_commands.json", "[]\n")
        return
    end
    local wd = shell_chomp(cook.sh("pwd"))
    local entries = {}
    for _, name in ipairs(targets) do
        local info = cook.import(name)
        if info and info.compile_info then
            local ci = info.compile_info
            ci.target_name = name
            for _, src in ipairs(ci.sources or {}) do
                entries[#entries + 1] = {
                    directory = wd,
                    command   = format_command(src, ci),
                    file      = src,
                }
            end
        end
    end
    fs.write("compile_commands.json", cjson.encode(entries) .. "\n")
end

return M
