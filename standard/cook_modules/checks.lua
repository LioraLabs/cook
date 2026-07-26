-- standard/cook_modules/checks.lua
--
-- Repo-local checks for the Cook Standard:
--   * checks.lint_keywords()        — flag lowercase RFC 2119 keywords in normative chapters (skips fenced blocks and inline code).
--
-- `lint_keywords` is a target-maker: called from a recipe body, it registers a
-- test unit (cook.add_test, §22.4) on the enclosing recipe, so `cook test`
-- reports it and an unchanged corpus does not re-scan. `suite` defaults to the
-- enclosing recipe's qualified name, and the normative glob set stays the
-- module's business — the recipe declares no ingredients.
--
-- `against_tag` used to live here as a shell pipeline assembled with
-- table.concat and handed to cook.exec. It is now scripts/against-tag.sh,
-- invoked directly by the `against-tag` chore: same commands, in a file a shell
-- can run and a reader can read.

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
    -- Strip inline code spans (single backticks) before pattern matching
    local stripped = line:gsub("`[^`]*`", "")
    for _, pat in ipairs(KEYWORD_PATTERNS) do
        if stripped:find(pat) then return true end
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

return checks
