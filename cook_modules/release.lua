-- cook_modules/release.lua — repo-wide release helpers.
--
-- Functions:
--   release.bump_claim(version)  — mirror a Standard version into every
--                                  in-repo claim site (cook-lang Rust
--                                  constant + 3 markdown files +
--                                  tree-sitter-cook grammar.js header,
--                                  package.json description,
--                                  tree-sitter.json description).
--                                  Both-phase: pure fs.* + string.
--
--   release.cut(version)         — cut a host-target release: build cook
--                                  via `cook package`, tag the commit,
--                                  push to origin, and upload the host's
--                                  tarball + a merged release-wide
--                                  checksums file to the LioraLabs/cook
--                                  GitHub release. Idempotent across
--                                  machines: re-running on the same
--                                  VERSION from a different host appends
--                                  that host's artifact and rebuilds the
--                                  canonical checksums file.
--                                  Register-phase only.

local M = {}

local REPO = "LioraLabs/cook"
local DIST = "cli/target/dist"

-- pcall wrapper: cook.sh raises on non-zero; we want a boolean for control
-- flow. On failure, returns (false, err) where `err` is the cook.sh error
-- message (the COOK_CMD_FAILED:... payload) so callers can log diagnostics.
local function try_sh(cmd)
    local ok, out_or_err = pcall(cook.sh, cmd)
    if ok then return true, out_or_err:gsub("%s+$", "") end
    return false, out_or_err
end

local function rstrip(s) return (s:gsub("%s+$", "")) end

-- ── bump_claim (unchanged behaviour, lightly tidied) ────────────────────────

local function read_version_default()
    if not fs.exists("standard/VERSION") then
        error("release.bump_claim: standard/VERSION not found "
            .. "(call from repo root, or pass an explicit version)")
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
        error("release.bump_claim: pattern not found in " .. path
            .. " (claim site may have moved; check the regex)")
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
        error("release.bump_claim: version '" .. version .. "' is not MAJOR.MINOR; "
            .. "pass --set VERSION=X.Y")
    end

    print("[release.bump_claim] cook-lang + tree-sitter-cook → v" .. version)

    -- 1. Rust source constant.
    rewrite_file(
        "cli/crates/cook-lang/src/lib.rs",
        'pub const COOK_STANDARD_VERSION: &str = "[^"]*"',
        'pub const COOK_STANDARD_VERSION: &str = "' .. version .. '"',
        "cook-lang Rust source")

    -- 2. Markdown claim line in three READMEs.
    for _, path in ipairs({
        "cli/crates/cook-lang/README.md",
        "cli/crates/cook-lang/CONFORMANCE.md",
        "README.md",
    }) do
        rewrite_file(
            path,
            "claims %*%*Cook Standard v[0-9%.]+%*%*",
            "claims **Cook Standard v" .. version .. "**",
            "README claim")
    end

    -- 3. tree-sitter-cook grammar.js header comment.
    rewrite_file(
        "tree-sitter-cook/grammar.js",
        "tree%-sitter%-cook claims conformance with Cook Standard v[0-9%.]+",
        "tree-sitter-cook claims conformance with Cook Standard v" .. version,
        "tree-sitter-cook grammar.js header")

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
            "tree-sitter-cook " .. path:match("[^/]+$"))
    end

    print("[release.bump_claim] done — review with `git diff` and commit")
end

-- ── cut: helpers ────────────────────────────────────────────────────────────

local function preflight_clean_tree()
    local ok = try_sh("git diff-index --quiet HEAD --")
    if not ok then
        error("[release.cut] working tree has uncommitted changes; commit or stash first")
    end
end

local function preflight_gh_auth()
    local ok = try_sh("gh auth status >/dev/null 2>&1")
    if not ok then
        error("[release.cut] gh CLI not authenticated; run 'gh auth login'")
    end
end

local function host_target()
    local triple = rstrip(cook.sh("rustc -vV | sed -n 's/host: //p'"))
    if triple == "" then
        error("[release.cut] could not resolve rustc host triple")
    end
    local os_id
    if triple:find("apple%-darwin") then os_id = "darwin"
    elseif triple:find("linux") then os_id = "linux"
    else
        error("[release.cut] unsupported host OS in '" .. triple
            .. "' (Phase 1: linux, darwin)")
    end

    local arch
    if triple:find("^x86_64%-") then arch = "amd64"
    elseif triple:find("^aarch64%-") then arch = "arm64"
    else
        error("[release.cut] unsupported host arch in '" .. triple
            .. "' (Phase 1: amd64, arm64)")
    end

    return triple, os_id, arch
end

local function ensure_tag(version)
    local has_local = try_sh("git rev-parse --verify --quiet '" .. version .. "'")
    if has_local then
        print("[release.cut] tag " .. version .. " already exists locally")
    else
        cook.sh(string.format("git tag -a '%s' -m 'cook %s'", version, version))
    end

    local on_origin = try_sh(string.format(
        "git ls-remote --exit-code --tags origin '%s' >/dev/null 2>&1", version))
    if on_origin then
        print("[release.cut] tag " .. version .. " already on origin")
    else
        cook.sh("git push origin '" .. version .. "'")
    end
end

local function reconcile_sums(version, sums_name, host_line, tarball_name)
    local sums_dir = rstrip(cook.sh("mktemp -d"))
    local sums_path = sums_dir .. "/" .. sums_name

    local exists = try_sh(string.format(
        "gh release view '%s' --repo %s >/dev/null 2>&1", version, REPO))

    if exists then
        -- Pull the current sums; ignore failure (file may not be present yet).
        try_sh(string.format(
            "gh release download '%s' --repo %s --pattern '%s' -O '%s' 2>/dev/null",
            version, REPO, sums_name, sums_path))
    end

    -- Drop any prior line for THIS host's tarball, append new, sort.
    local lines = {}
    if fs.exists(sums_path) and fs.size(sums_path) > 0 then
        for line in fs.read(sums_path):gmatch("[^\n]+") do
            local fname = line:match("^%S+%s+(%S+)$")
            if fname ~= tarball_name then
                table.insert(lines, line)
            end
        end
    end
    table.insert(lines, host_line)
    table.sort(lines, function(a, b)
        return (a:match("%s(%S+)$") or "") < (b:match("%s(%S+)$") or "")
    end)
    fs.write(sums_path, table.concat(lines, "\n") .. "\n")

    return sums_path, exists
end

local function upload(version, tarball_path, sums_path, release_exists)
    if release_exists then
        print("[release.cut] release exists; merging into existing artifacts")
        cook.sh(string.format(
            "gh release upload '%s' --repo %s --clobber '%s' '%s'",
            version, REPO, tarball_path, sums_path))
    else
        print("[release.cut] creating release " .. version)
        local notes = "Phase 1 install layout. Host targets uploaded as cuts arrive "
            .. "from each machine; remaining targets land via CI follow-up "
            .. "(see SHI-176 M1.2b)."
        cook.sh(string.format(
            "gh release create '%s' --repo %s --title 'cook %s' --notes %q '%s' '%s'",
            version, REPO, version, notes, tarball_path, sums_path))
    end
end

-- ── cut ─────────────────────────────────────────────────────────────────────

function M.cut(version)
    if not version or version == "" then
        error("[release.cut] pass --set VERSION=vX.Y.Z (got empty)")
    end
    -- Canonicalise: ensure leading 'v'.
    if not version:match("^v") then version = "v" .. version end

    print("[release.cut] version: " .. version)

    preflight_clean_tree()
    preflight_gh_auth()

    local triple, os_id, arch = host_target()
    print(string.format("[release.cut] host: %s → %s-%s", triple, os_id, arch))

    local tarball_name = string.format("cook-%s-%s-%s.tar.gz", version, os_id, arch)
    local sums_name = string.format("cook-%s-checksums.txt", version)
    local tarball_path = DIST .. "/" .. tarball_name

    print("[release.cut] packaging via cook package...")
    cook.sh("rm -rf " .. DIST)
    cook.sh(string.format(
        "cook --set VERSION=%s --set TARGET=%s package", version, triple))

    if not fs.exists(tarball_path) then
        error("[release.cut] expected tarball not found at " .. tarball_path)
    end
    if not fs.exists(tarball_path .. ".sha256") then
        error("[release.cut] expected sha256 not found at " .. tarball_path .. ".sha256")
    end

    local host_line = rstrip(fs.read(tarball_path .. ".sha256"))
    if host_line == "" then
        error("[release.cut] empty .sha256 sibling at " .. tarball_path .. ".sha256")
    end

    ensure_tag(version)

    local sums_path, release_exists = reconcile_sums(
        version, sums_name, host_line, tarball_name)

    upload(version, tarball_path, sums_path, release_exists)

    print("[release.cut] done: " .. tarball_name .. " on " .. REPO .. "@" .. version)
end

return M
