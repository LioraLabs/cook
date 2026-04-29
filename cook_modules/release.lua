-- cook_modules/release.lua — repo-wide release helpers.
--
-- Exposes the build-script logic that the top-level Cookfile used to
-- inline as long sed pipelines. Functions here are both-phase by
-- construction: they only call fs.* and string operations, so the same
-- code works whether the caller is in the declarative region (bare
-- `release.bump_claim(...)`) or the imperative region (`> release.bump_claim(...)`).
--
-- Functions:
--   release.bump_claim(version)  — mirror a Standard version into every
--                                  in-repo claim site (cook-lang Rust
--                                  constant + 3 markdown files +
--                                  tree-sitter-cook grammar.js header,
--                                  package.json description,
--                                  tree-sitter.json description).

local M = {}

local function rstrip(s)
    return (s:gsub("%s+$", ""))
end

local function read_version_default()
    if not fs.exists("standard/VERSION") then
        error(
            "release.bump_claim: standard/VERSION not found "
                .. "(call from repo root, or pass an explicit version)"
        )
    end
    return rstrip(fs.read("standard/VERSION"))
end

local function rewrite_file(path, pattern, replacement, label)
    if not fs.exists(path) then
        error("release.bump_claim: " .. label .. " not found at " .. path)
    end
    local before = fs.read(path)
    local after, n = before:gsub(pattern, replacement)
    if n == 0 then
        error(
            "release.bump_claim: pattern not found in " .. path
                .. " (claim site may have moved; check the regex)"
        )
    end
    fs.write(path, after)
    return n
end

function M.bump_claim(version)
    if not version or version == "" then
        version = read_version_default()
    end
    -- Accept either "0.3" or "v0.3"; canonicalise to bare digits.
    version = version:gsub("^v", "")
    if not version:match("^[0-9]+%.[0-9]+$") then
        error(
            "release.bump_claim: version '" .. version .. "' is not MAJOR.MINOR; "
                .. "pass --set VERSION=X.Y"
        )
    end

    print("[release.bump_claim] cook-lang + tree-sitter-cook → v" .. version)

    -- 1. Rust source constant.
    rewrite_file(
        "cli/crates/cook-lang/src/lib.rs",
        'pub const COOK_STANDARD_VERSION: &str = "[^"]*"',
        'pub const COOK_STANDARD_VERSION: &str = "' .. version .. '"',
        "cook-lang Rust source"
    )

    -- 2. Markdown claim line in three READMEs.
    local readmes = {
        "cli/crates/cook-lang/README.md",
        "cli/crates/cook-lang/CONFORMANCE.md",
        "README.md",
    }
    for _, path in ipairs(readmes) do
        rewrite_file(
            path,
            "claims %*%*Cook Standard v[0-9%.]+%*%*",
            "claims **Cook Standard v" .. version .. "**",
            "README claim"
        )
    end

    -- 3. tree-sitter-cook grammar.js header comment.
    rewrite_file(
        "tree-sitter-cook/grammar.js",
        "tree%-sitter%-cook claims conformance with Cook Standard v[0-9%.]+",
        "tree-sitter-cook claims conformance with Cook Standard v" .. version,
        "tree-sitter-cook grammar.js header"
    )

    -- 4. tree-sitter-cook package.json + tree-sitter.json description string.
    -- The numeric `version` field uses npm semver and is intentionally not
    -- bumped here — tree-sitter-cook has its own release cadence.
    for _, path in ipairs({
        "tree-sitter-cook/package.json",
        "tree-sitter-cook/tree-sitter.json",
    }) do
        rewrite_file(
            path,
            "claims Cook Standard v[0-9%.]+",
            "claims Cook Standard v" .. version,
            "tree-sitter-cook " .. path:match("[^/]+$")
        )
    end

    print("[release.bump_claim] done — review with `git diff` and commit")
end

return M
