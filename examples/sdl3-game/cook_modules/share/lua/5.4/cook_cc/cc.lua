local toolchain = require("cook_cc.toolchain")

local M = {}

local function is_c(source) return source:match("%.[cC]$") ~= nil end

local function compiler_for(source)
    local c = toolchain.get_compiler()
    return is_c(source) and c.cc or c.cxx
end

function M.compile(source, opts)
    opts = opts or {}
    if not fs.exists(source) then
        error("[cc.compile] source file not found: " .. source, 2)
    end
    local target_name = opts.target_name or "default"
    local stem = path.stem(source)
    local obj_dir = "build/obj/" .. target_name
    local obj_out = opts.output or (obj_dir .. "/" .. stem .. ".o")
    local dep_dir = ".cook/deps/" .. target_name
    local dep_file = dep_dir .. "/" .. stem .. ".d"

    fs.mkdir_p(obj_dir)
    fs.mkdir_p(dep_dir)

    local flags = { "-c", "-MMD", "-MF", dep_file }

    local std = opts.standard or toolchain.get_default_standard()
    if std and not is_c(source) then
        flags[#flags + 1] = "-std=" .. std
    end

    for _, inc in ipairs(opts.includes or {}) do flags[#flags + 1] = "-I" .. inc end
    for _, def in ipairs(opts.defines  or {}) do flags[#flags + 1] = "-D" .. def end

    local wflags = toolchain.warning_flags(opts.warnings)
    if wflags ~= "" then flags[#flags + 1] = wflags end

    if opts.fpic then flags[#flags + 1] = "-fPIC" end
    if opts.extra_cflags and opts.extra_cflags ~= "" then
        flags[#flags + 1] = opts.extra_cflags
    end

    -- Trailing space ensures assertions like " -c " match even when -c is the last token.
    local cmd = compiler_for(source) .. " " .. table.concat(flags, " ")
        .. " " .. source .. " -o " .. obj_out .. " "

    cook.add_unit({
        inputs = { source },
        output = obj_out,
        command = cmd,
        discovered_inputs = { from = dep_file, format = "make" },
    })

    return obj_out
end

function M.archive(objects, output)
    fs.mkdir_p(path.dir(output))
    -- Trailing space ensures assertions like "x.o " match the last token.
    local cmd = "ar rcs " .. output .. " " .. table.concat(objects, " ") .. " "
    cook.add_unit({
        inputs = objects,
        output = output,
        command = cmd,
    })
    return output
end

function M.link(objects, output, opts)
    opts = opts or {}
    fs.mkdir_p(path.dir(output))
    local cxx = toolchain.get_compiler().cxx
    local parts = { cxx, table.concat(objects, " "), "-o", output }
    for _, lib in ipairs(opts.system_libs or {}) do
        parts[#parts + 1] = "-l" .. lib
    end
    if cook.platform.os == "macos" then
        for _, fw in ipairs(opts.frameworks or {}) do
            parts[#parts + 1] = "-framework"
            parts[#parts + 1] = fw
        end
    end
    if opts.extra_ldflags and opts.extra_ldflags ~= "" then
        parts[#parts + 1] = opts.extra_ldflags
    end
    if opts.shared then parts[#parts + 1] = "-shared" end
    -- Trailing space ensures assertions like " -lpthread " match the last token.
    local cmd = table.concat(parts, " ") .. " "

    cook.add_unit({
        inputs = objects,
        output = output,
        command = cmd,
    })
    return output
end

return M
