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
--   release.cut(version)         — gate on a clean tree, an in-sync
--                                  conformance claim, and a green `cook test`,
--                                  then bump manifest, commit, tag, push — CI
--                                  builds and publishes (release.yml).

local M = {}

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

local function rewrite_file(path, pattern, replacement, label, err_prefix)
    err_prefix = err_prefix or "release.bump_claim"
    if not fs.exists(path) then
        error(err_prefix .. ": " .. label .. " not found at " .. path)
    end
    local before = fs.read(path)
    local after, n = before:gsub(pattern, replacement)
    if n == 0 then
        error(err_prefix .. ": pattern not found in " .. path
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
    -- The root README.md dropped its claim line in the 2026-07-19 topcoat
    -- restructure; cook-lang's README/CONFORMANCE carry the claim now.
    for _, path in ipairs({
        "cli/crates/cook-lang/README.md",
        "cli/crates/cook-lang/CONFORMANCE.md",
    }) do
        rewrite_file(
            path,
            "claims %*%*Cook Standard v[0-9%.]+%*%*",
            "claims **Cook Standard v" .. version .. "**",
            "README claim")
    end

    -- 3. tree-sitter-cook grammar.js header comment.
    -- Pattern tracks the wording actually in the file ("Conforms to Cook
    -- Standard vX.Y"); the previous regex looked for a phrasing no claim site
    -- uses, so this step had been failing the whole chore.
    rewrite_file(
        "tree-sitter-cook/grammar.js",
        -- MAJOR%.MINOR, not [0-9%.]+ — the latter is greedy and would swallow
        -- the sentence's closing period along with the version.
        "Conforms to Cook Standard v[0-9]+%.[0-9]+",
        "Conforms to Cook Standard v" .. version,
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
            "conforming to Cook Standard v[0-9%.]+",
            "conforming to Cook Standard v" .. version,
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

-- The claim sites and standard/VERSION drifted three cuts apart (v0.15 vs
-- v0.18) because nothing checked them at the one moment it matters. `cut` is
-- that moment: the tag it pushes is what a downstream implementor reads the
-- claim off. Cheap, so it runs before the expensive gate below.
local function preflight_claim_in_sync()
    local declared = rstrip(fs.read("standard/VERSION"))
    local claimed = fs.read("cli/crates/cook-lang/src/lib.rs")
        :match('pub const COOK_STANDARD_VERSION: &str = "([^"]*)"')
    if claimed ~= declared then
        error(string.format(
            "[release.cut] conformance claim is stale: cook-lang claims v%s, "
            .. "standard/VERSION says v%s — run `cook bump-claim` and commit",
            tostring(claimed), declared))
    end
    -- grammar.js is the claim site bump_claim was silently failing to rewrite
    -- for several cuts, so it is checked explicitly rather than assumed to
    -- follow the Rust constant.
    local grammar = fs.read("tree-sitter-cook/grammar.js")
        :match("Conforms to Cook Standard v([0-9]+%.[0-9]+)")
    if grammar ~= declared then
        error(string.format(
            "[release.cut] tree-sitter-cook claims v%s, standard/VERSION says "
            .. "v%s — run `cook bump-claim` and commit",
            tostring(grammar), declared))
    end
end

-- The test gate. `cut` pushes a tag, and that tag fires release.yml's
-- build+publish matrix — by the time CI reports a failure the version is
-- already public, so the gate has to be here, ahead of every mutation.
--
-- It runs the same two commands ci.yml runs, in the same order and against a
-- release binary built from the tree being tagged: `cook test` is self-hosted,
-- so gating on a cook from PATH would test whatever is installed rather than
-- what is about to ship. There is no skip flag on purpose.
local function preflight_tests()
    print("[release.cut] building the release binary for the test gate...")
    cook.sh("cargo build --release -p cook-cli --manifest-path cli/Cargo.toml")

    local cook_bin = "cli/target/release/cook"
    if not fs.exists(cook_bin) then
        error("[release.cut] " .. cook_bin .. " not found after cargo build")
    end

    print("[release.cut] running `cook test` (ci.yml's gate)...")
    local ok, err = try_sh("./" .. cook_bin .. " test")
    if not ok then
        error("[release.cut] `cook test` failed; nothing was committed, tagged "
            .. "or pushed. Fix the failures and rerun.\n" .. tostring(err))
    end
    print("[release.cut] test gate green")
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

-- ── cut ─────────────────────────────────────────────────────────────────────

function M.cut(version)
    if not version or version == "" then
        error("[release.cut] pass --set VERSION=vX.Y.Z (got empty)")
    end
    -- Canonicalise: ensure leading 'v'.
    if not version:match("^v") then version = "v" .. version end
    -- vX.Y.Z, optionally followed by a '-' prerelease suffix (e.g. v1.2.3-rc1).
    local core, suffix = version:match("^(v%d+%.%d+%.%d+)(.-)$")
    if not core or not (suffix == "" or suffix:match("^%-[%w%.%-]+$")) then
        error("[release.cut] version '" .. version .. "' is not vX.Y.Z"
            .. " (with optional -suffix); pass --set VERSION=vX.Y.Z")
    end

    print("[release.cut] version: " .. version)

    preflight_clean_tree()
    preflight_claim_in_sync()
    preflight_tests()

    local bare_version = version:gsub("^v", "")
    local manifest_path = "cli/Cargo.toml"
    if not fs.exists(manifest_path) then
        error("[release.cut] " .. manifest_path .. " not found (call from repo root)")
    end
    local manifest_before = fs.read(manifest_path)
    local current_version = manifest_before:match('%[workspace%.package%]\nversion = "([^"]*)"')

    if current_version == bare_version then
        print("[release.cut] " .. manifest_path .. " already at " .. bare_version
            .. "; skipping bump+commit (rerun)")
    else
        rewrite_file(
            manifest_path,
            '(%[workspace%.package%]\n)version = "[^"]*"',
            '%1version = "' .. bare_version .. '"',
            "[workspace.package] version",
            "[release.cut]")

        print("[release.cut] syncing cli/Cargo.lock...")
        cook.sh("(cd cli && cargo update --workspace --offline)")

        cook.sh(string.format(
            "git add cli/Cargo.toml cli/Cargo.lock && git commit -m 'release: %s'",
            version))
    end

    cook.sh("git push origin HEAD")
    ensure_tag(version)

    print("[release.cut] done: tag " .. version .. " pushed — release.yml will build and publish")
end

return M
