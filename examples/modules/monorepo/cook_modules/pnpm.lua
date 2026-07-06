-- pnpm cook_module: orchestrate pnpm workspace builds through Cook's DAG
-- with correct catalog: specifier resolution.

local pnpm = {}

pnpm.state = {
    workspace = nil,    -- parsed pnpm-workspace.yaml
    packages = {},      -- name -> {name, dir, json, workspace_deps}
    initialized = false,
}

-- ---------------------------------------------------------------------------
-- Specifier resolution
-- ---------------------------------------------------------------------------

--- Check if a dependency specifier routes through the workspace.
--- The caller already verified dep_name is a known workspace package,
--- so we just need to confirm the specifier is workspace:* or a catalog
--- entry that references this dep (pnpm links workspace packages when
--- the catalog version range matches the local package version).
local function is_workspace_specifier(dep_name, spec, workspace)
    -- workspace:* or workspace:^ -> always internal
    if spec:match("^workspace:") then
        return true
    end

    -- catalog: (default catalog) -> internal if catalog has an entry for this dep
    if spec == "catalog:" then
        if workspace.catalog and workspace.catalog[dep_name] then
            return true
        end
        return false
    end

    -- catalog:name (named catalog) -> internal if the named catalog has this dep
    local cat_name = spec:match("^catalog:(.+)$")
    if cat_name then
        local cat = workspace.catalogs and workspace.catalogs[cat_name]
        if not cat then
            local available = {}
            if workspace.catalogs then
                for k, _ in pairs(workspace.catalogs) do available[#available + 1] = k end
                table.sort(available)
            end
            error("pnpm: unknown catalog '" .. cat_name .. "' referenced by dependency '"
                .. dep_name .. "' (available: " .. table.concat(available, ", ") .. ")")
        end
        return cat[dep_name] ~= nil
    end

    return false
end

local function collect_workspace_deps(pkg_json, workspace, all_pkg_names)
    local deps = {}
    local dep_sections = { pkg_json.dependencies, pkg_json.devDependencies }
    for _, section in ipairs(dep_sections) do
        if section then
            for dep_name, spec in pairs(section) do
                if all_pkg_names[dep_name] and is_workspace_specifier(dep_name, spec, workspace) then
                    deps[#deps + 1] = dep_name
                end
            end
        end
    end
    table.sort(deps)
    return deps
end

-- ---------------------------------------------------------------------------
-- Topological sort
-- ---------------------------------------------------------------------------

local function topo_sort(packages, task)
    local order = {}
    local visited = {}
    local visiting = {}

    local function visit(name)
        if visited[name] then return end
        if visiting[name] then
            error("pnpm: dependency cycle detected involving '" .. name .. "'")
        end
        visiting[name] = true
        local pkg = packages[name]
        if pkg then
            for _, dep_name in ipairs(pkg.workspace_deps) do
                local dep = packages[dep_name]
                if dep and dep.json.scripts and dep.json.scripts[task] then
                    visit(dep_name)
                end
            end
        end
        visiting[name] = nil
        visited[name] = true
        if pkg and pkg.json.scripts and pkg.json.scripts[task] then
            order[#order + 1] = name
        end
    end

    local names = {}
    for name, _ in pairs(packages) do names[#names + 1] = name end
    table.sort(names)
    for _, name in ipairs(names) do
        visit(name)
    end
    return order
end

-- ---------------------------------------------------------------------------
-- Input list construction
-- ---------------------------------------------------------------------------

local EXCLUDE_DIRS = {
    "node_modules", ".cook", "dist", "build",
    ".next", "out", "coverage", ".turbo", ".parcel-cache",
}

--- Check if a file path traverses or ends at an excluded directory.
--- Handles both "foo/dist/bar" (inside) and "foo/dist" (the dir itself).
local function is_excluded(path)
    for _, dir in ipairs(EXCLUDE_DIRS) do
        local search = "/" .. dir
        local pos = path:find(search, 1, true)
        if pos then
            local after = pos + #search
            if after > #path or path:sub(after, after) == "/" then
                return true
            end
        end
    end
    return false
end

local function build_input_list(pkg, task, packages)
    local inputs = {}

    local all_files = fs.glob(pkg.dir .. "/**/*")
    for _, f in ipairs(all_files) do
        if not is_excluded(f) then
            inputs[#inputs + 1] = f
        end
    end

    inputs[#inputs + 1] = "pnpm-lock.yaml"
    inputs[#inputs + 1] = "pnpm-workspace.yaml"
    if fs.exists(".npmrc") then
        inputs[#inputs + 1] = ".npmrc"
    end

    for _, dep_name in ipairs(pkg.workspace_deps) do
        local dep = packages[dep_name]
        if dep and dep.json.scripts and dep.json.scripts[task] then
            inputs[#inputs + 1] = dep.dir .. "/.cook/" .. task .. ".done"
        end
    end

    return inputs
end

-- ---------------------------------------------------------------------------
-- Public API
-- ---------------------------------------------------------------------------

function pnpm.init()
    if pnpm.state.initialized then return end

    local ws_path = "pnpm-workspace.yaml"
    if not fs.exists(ws_path) then
        error("pnpm: pnpm-workspace.yaml not found in working directory")
    end
    local ws_str = fs.read(ws_path)
    pnpm.state.workspace = cook.yaml_decode(ws_str)

    local ws = pnpm.state.workspace
    local pkg_patterns = ws.packages or {}
    local pkg_dirs = {}
    for _, pattern in ipairs(pkg_patterns) do
        local dirs = fs.glob(pattern)
        for _, d in ipairs(dirs) do
            if fs.exists(d .. "/package.json") then
                pkg_dirs[#pkg_dirs + 1] = d
            end
        end
    end

    local all_pkg_names = {}
    local raw_packages = {}
    for _, dir in ipairs(pkg_dirs) do
        local json_str = fs.read(dir .. "/package.json")
        local ok, pkg_json = pcall(cook.json_decode, json_str)
        if not ok then
            error("pnpm: invalid JSON in " .. dir .. "/package.json: " .. tostring(pkg_json))
        end
        if pkg_json.name then
            all_pkg_names[pkg_json.name] = true
            raw_packages[#raw_packages + 1] = { name = pkg_json.name, dir = dir, json = pkg_json }
        end
    end

    for _, pkg in ipairs(raw_packages) do
        pkg.workspace_deps = collect_workspace_deps(pkg.json, ws, all_pkg_names)
        pnpm.state.packages[pkg.name] = pkg
    end

    pnpm.state.initialized = true
end

function pnpm.install()
    pnpm.init()
    cook.add_unit({
        inputs = { "pnpm-workspace.yaml", "pnpm-lock.yaml", "package.json" },
        output = ".cook/install.done",
        command = "pnpm install && mkdir -p .cook && touch .cook/install.done",
    })
end

function pnpm.run(task)
    pnpm.init()

    local packages = pnpm.state.packages
    local order = topo_sort(packages, task)

    for _, name in ipairs(order) do
        local pkg = packages[name]
        local inputs = build_input_list(pkg, task, packages)
        local marker_dir = pkg.dir .. "/.cook"
        local marker = marker_dir .. "/" .. task .. ".done"

        cook.add_unit({
            inputs = inputs,
            output = marker,
            command = "pnpm --filter '" .. pkg.name .. "' run " .. task
                .. " && mkdir -p " .. marker_dir
                .. " && touch " .. marker,
        })
    end
end

return pnpm
