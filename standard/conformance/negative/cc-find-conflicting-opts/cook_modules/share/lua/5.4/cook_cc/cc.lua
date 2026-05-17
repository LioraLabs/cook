local toolchain = require("cook_cc.toolchain")

local M = {}

local function is_c(source) return source:match("%.[cC]$") ~= nil end

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

    -- Compiler comes from the cc:compiler:<override> probe at execute time.
    toolchain.ensure_probe_registered()
    local cc_probe_key = toolchain.get_probe_key()
    local cc_field = is_c(source) and "cc" or "cxx"
    local compiler_sigil = "$<" .. cc_probe_key .. "." .. cc_field .. ">"

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

    local needs = opts.needs or {}
    local probes = { cc_probe_key }
    local cflags_sigils = {}
    for _, n in ipairs(needs) do
        probes[#probes + 1] = "cc:find:" .. n
        cflags_sigils[#cflags_sigils + 1] = "$<cc:find:" .. n .. ".cflags>"
    end

    local cmd = compiler_sigil .. " " .. table.concat(flags, " ")
        .. " " .. source .. " -o " .. obj_out
    if #cflags_sigils > 0 then
        cmd = cmd .. " " .. table.concat(cflags_sigils, " ")
    end
    cmd = cmd .. " "

    cook.add_unit({
        inputs            = { source },
        output            = obj_out,
        command           = cmd,
        probes            = probes,
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

    toolchain.ensure_probe_registered()
    local cc_probe_key = toolchain.get_probe_key()
    local compiler_sigil = "$<" .. cc_probe_key .. ".cxx>"

    local needs = opts.needs or {}
    local probes = { cc_probe_key }
    local libs_sigils = {}
    for _, n in ipairs(needs) do
        probes[#probes + 1] = "cc:find:" .. n
        libs_sigils[#libs_sigils + 1] = "$<cc:find:" .. n .. ".libs>"
    end

    local parts = { compiler_sigil, table.concat(objects, " "), "-o", output }
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
    if #libs_sigils > 0 then
        parts[#parts + 1] = table.concat(libs_sigils, " ")
    end

    -- Trailing space ensures assertions like " -lpthread " match the last token.
    local cmd = table.concat(parts, " ") .. " "

    cook.add_unit({
        inputs  = objects,
        output  = output,
        command = cmd,
        probes  = probes,
    })
    return output
end

return M
