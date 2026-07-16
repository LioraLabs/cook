-- standard/cook_modules/checks.lua
--
-- Repo-local checks for the Cook Standard:
--   * checks.lint_keywords()        — flag lowercase RFC 2119 keywords in normative chapters.
--   * checks.against_tag(version)   — verify cook-lang against the conformance corpus
--                                     materialized from the cs-standard/<version> git tag.
--
-- `lint_keywords` is a target-maker: called from a recipe body, it registers a
-- test unit (cook.add_test, §22.4) on the enclosing recipe, so `cook test`
-- reports it and an unchanged corpus does not re-scan. `suite` defaults to the
-- enclosing recipe's qualified name, and the normative glob set stays the
-- module's business — the recipe declares no ingredients.
--
-- `against_tag` still executes synchronously during the register phase via
-- cook.sh / fs.* — it is not a work unit and fails the chore directly via
-- error().

local checks = {}

-- ---------------------------------------------------------------------------
-- checks.lint_keywords
-- ---------------------------------------------------------------------------

local NORMATIVE_GLOBS = {
    "src/content/docs/0[0-9]-*.mdx",
    "src/content/docs/appendix/A-*.mdx",
}

-- Word-boundary mirror of grep's `\b...\b`: underscore counts as a word
-- character, so `should_fail` does not match `should`.
local KEYWORD_PATTERNS = {
    "%f[%w_]must%f[^%w_]",
    "%f[%w_]shall%f[^%w_]",
    "%f[%w_]should%f[^%w_]",
    "%f[%w_]may%f[^%w_]",
}

local function line_matches_keyword(line)
    for _, pat in ipairs(KEYWORD_PATTERNS) do
        if line:find(pat) then return true end
    end
    return false
end

local function scan_file_for_keywords(path_)
    local content = fs.read(path_)
    local hits = {}
    local in_fence = false
    local line_no = 0
    -- Append a sentinel newline so the iterator yields the final unterminated line.
    for line in (content .. "\n"):gmatch("([^\n]*)\n") do
        line_no = line_no + 1
        if line:match("^```") then
            in_fence = not in_fence
        elseif not in_fence and line_matches_keyword(line) then
            hits[#hits + 1] = line_no .. ":" .. line
        end
    end
    return hits
end

local function normative_files()
    local files = {}
    for _, glob in ipairs(NORMATIVE_GLOBS) do
        for _, p in ipairs(fs.glob(glob)) do
            files[#files + 1] = p
        end
    end
    table.sort(files)
    return files
end

-- Register-phase. Call from inside a recipe body: cook.add_test attaches the
-- unit to the enclosing recipe and has no body slot to attach to at top level.
-- `inputs` keys the scan, so editing a non-normative chapter does not re-run it.
function checks.lint_keywords()
    cook.add_test({
        lua_code = 'require("checks").scan_keywords()',
        inputs = normative_files(),
    })
end

-- Execute-phase half, reached via the `lua_code` above. It re-requires the
-- module because `use`-bound globals are register-phase only. Raising a Lua
-- error is how a lua_code test reports failure (§22.4).
function checks.scan_keywords()
    local files = normative_files()

    local files_with_hits = 0
    for _, f in ipairs(files) do
        local hits = scan_file_for_keywords(f)
        if #hits > 0 then
            files_with_hits = files_with_hits + 1
            print("== " .. f .. " ==")
            for _, h in ipairs(hits) do print(h) end
        end
    end

    if files_with_hits > 0 then
        print("")
        error(
            "check-normative-keywords: lowercase RFC 2119 keywords found in "
                .. files_with_hits
                .. " file(s). Promote to all-caps if the clause is binding, or "
                .. "reword to remove the keyword if the clause is descriptive."
        )
    end
    print("check-normative-keywords: OK")
end

-- ---------------------------------------------------------------------------
-- checks.against_tag
-- ---------------------------------------------------------------------------

local function rstrip(s)
    return (s:gsub("%s+$", ""))
end

local function tag_exists(tag)
    local ok = pcall(function()
        cook.sh("git rev-parse --verify --quiet " .. tag)
    end)
    return ok
end

function checks.against_tag(version)
    if not version or version == "" then
        error("checks.against_tag: version required (e.g. '0.1' or 'v0.1')")
    end
    if version:sub(1, 1) ~= "v" then
        version = "v" .. version
    end
    local tag = "cs-standard/" .. version

    if not tag_exists(tag) then
        error("checks.against_tag: tag '" .. tag .. "' not found in this repository")
    end

    -- `git -C <repo_root>` so the pathspec resolves repo-relative even
    -- though the recipe runs with cwd = standard/. The corpus path is
    -- absolute because cargo test changes the test's working directory
    -- to the crate root, so a relative path would no longer resolve.
    local repo_root = rstrip(cook.sh("git rev-parse --show-toplevel"))
    local tmpdir = repo_root .. "/standard/.cook/conformance-" .. version
    local corpus = tmpdir .. "/conformance"

    -- Setup, test, and cleanup are recorded as one non-cached unit so
    -- cargo test's output streams to the user during the execute phase.
    local pipeline = table.concat({
        "set -e",
        "rm -rf " .. tmpdir,
        "mkdir -p " .. tmpdir,
        "git -C " .. repo_root .. " archive " .. tag .. " standard/conformance"
            .. " | tar -x -C " .. tmpdir .. " --strip-components=1",
        "test -d " .. corpus .. "/positive"
            .. " || { echo 'checks.against_tag: tag did not contain standard/conformance/positive' >&2; exit 1; }",
        "echo 'Running cook-lang conformance harness against " .. tag .. "'",
        "echo 'Corpus: " .. corpus .. "'",
        "env COOK_CONFORMANCE_CORPUS=" .. corpus
            .. " cargo test --manifest-path " .. repo_root .. "/cli/Cargo.toml"
            .. " -p cook-lang --test conformance",
        "rm -rf " .. tmpdir,
    }, "\n")

    cook.exec(pipeline, 0)
end

return checks
