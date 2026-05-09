-- cook_modules/release.lua — repo-wide release helpers.
--
-- Exposes the build-script logic that the top-level Cookfile used to
-- inline as long sed pipelines. bump_claim is both-phase (fs.* + string
-- only); cut is imperative-only (uses cook.exec to drive cargo / git / gh).
--
-- Functions:
--   release.bump_claim(version)  — mirror a Standard version into every
--                                  in-repo claim site (cook-lang Rust
--                                  constant + 3 markdown files +
--                                  tree-sitter-cook grammar.js header,
--                                  package.json description,
--                                  tree-sitter.json description).
--
--   release.cut(version)         — cut a Phase 1 host-target release:
--                                  build cook, package via cook-xtask,
--                                  tag the commit, push to Gitea (which
--                                  mirrors to GitHub), and upload the
--                                  host's tarball + a merged checksums
--                                  file to the LioraLabs/cook GitHub
--                                  release. Idempotent across machines:
--                                  re-running on the same VERSION from a
--                                  different host appends that host's
--                                  artifact to the existing release and
--                                  rebuilds the canonical checksums file.
--                                  Imperative-phase only.

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

-- ── release.cut ─────────────────────────────────────────────────────────
-- Build cook for the host triple, package it via cook-xtask, tag the
-- current HEAD, push the tag, and upload the artifact + a merged
-- release-wide checksums file to the LioraLabs/cook GitHub release.
--
-- The release is created on first call and merged into on subsequent
-- calls (typically from a different host targeting the same VERSION).
-- The checksums file is the single source of truth for tarball hashes;
-- this function reconciles it on each upload by:
--   1. Downloading the existing checksums file (if any) from the release.
--   2. Removing any prior line for THIS host's tarball.
--   3. Appending this host's freshly-computed line.
--   4. Sorting and re-uploading with --clobber.
--
-- Pre-flights enforced:
--   • git working tree clean (no uncommitted changes).
--   • gh authenticated (gh auth status must succeed).
--   • Host triple maps to a Phase-1-supported (linux|darwin) × (amd64|arm64).
function M.cut(version)
    if not version or version == "" then
        error("release.cut: pass --set VERSION=vX.Y.Z (got empty)")
    end
    -- Canonicalise: ensure leading 'v'. The xtask + URL pattern (SHI-182)
    -- bake the leading 'v' into ${VERSION}, so we keep it through.
    if not version:match("^v") then
        version = "v" .. version
    end

    local pipeline = string.format(
        [[
set -euo pipefail
VERSION=%s
REPO=LioraLabs/cook
DIST=cli/target/dist
SUMS_NAME="cook-${VERSION}-checksums.txt"

echo "[release.cut] version: ${VERSION}"

# 1. Pre-flight: working tree clean
if ! git diff-index --quiet HEAD --; then
    echo "[release.cut] ERROR: working tree has uncommitted changes; commit or stash first" >&2
    exit 1
fi

# 2. Pre-flight: gh authenticated
gh auth status >/dev/null 2>&1 || {
    echo "[release.cut] ERROR: gh CLI not authenticated; run 'gh auth login'" >&2
    exit 1
}

# 3. Resolve host triple → ${OS}-${ARCH} (must match the URL pattern locked in SHI-182)
HOST_TRIPLE=$(rustc -vV | sed -n 's/host: //p')
case "${HOST_TRIPLE}" in
    *-apple-darwin*) OS=darwin ;;
    *-linux-*)       OS=linux ;;
    *) echo "[release.cut] ERROR: unsupported host OS in '${HOST_TRIPLE}' (Phase 1: linux, darwin)" >&2; exit 1 ;;
esac
case "${HOST_TRIPLE}" in
    x86_64-*)  ARCH=amd64 ;;
    aarch64-*) ARCH=arm64 ;;
    *) echo "[release.cut] ERROR: unsupported host arch in '${HOST_TRIPLE}' (Phase 1: amd64, arm64)" >&2; exit 1 ;;
esac
TARBALL_NAME="cook-${VERSION}-${OS}-${ARCH}.tar.gz"
echo "[release.cut] host: ${HOST_TRIPLE} → ${OS}-${ARCH}"

# 4. Build cook (release profile) and package it
echo "[release.cut] building cook..."
( cd cli && cargo build --release --bin cook )
rm -rf "${DIST}"
( cd cli && cargo xtask package --binary target/release/cook --version "${VERSION}" --target "${HOST_TRIPLE}" )
test -f "${DIST}/${TARBALL_NAME}" \
    || { echo "[release.cut] ERROR: expected tarball not found at ${DIST}/${TARBALL_NAME}" >&2; exit 1; }

# 5. Extract this host's checksum line for the merged file
HOST_SUMS_LINE=$(awk -v t="${TARBALL_NAME}" '$2==t {print; exit}' "${DIST}/cook-${VERSION}-checksums.txt")
test -n "${HOST_SUMS_LINE}" \
    || { echo "[release.cut] ERROR: no checksum entry for ${TARBALL_NAME} in xtask output" >&2; exit 1; }

# 6. Tag and push (idempotent: skip if already tagged locally / on origin)
if git rev-parse --verify --quiet "${VERSION}" >/dev/null; then
    echo "[release.cut] tag ${VERSION} already exists locally"
else
    git tag -a "${VERSION}" -m "cook ${VERSION}"
fi
if git ls-remote --exit-code --tags origin "${VERSION}" >/dev/null 2>&1; then
    echo "[release.cut] tag ${VERSION} already on origin"
else
    git push origin "${VERSION}"
fi

# 7. Reconcile the release-wide checksums file
MERGED_SUMS=$(mktemp)
trap 'rm -f "${MERGED_SUMS}"' EXIT

if gh release view "${VERSION}" --repo "${REPO}" >/dev/null 2>&1; then
    echo "[release.cut] release exists; merging into existing artifacts"
    # Pull the current release-wide sums; ignore failure (file may not be present yet).
    gh release download "${VERSION}" --repo "${REPO}" --pattern "${SUMS_NAME}" -O "${MERGED_SUMS}" 2>/dev/null || true
    # Drop any prior line for THIS host's tarball, append the fresh one, sort.
    if [ -s "${MERGED_SUMS}" ]; then
        grep -v "  ${TARBALL_NAME}\$" "${MERGED_SUMS}" > "${MERGED_SUMS}.tmp" || true
        mv "${MERGED_SUMS}.tmp" "${MERGED_SUMS}"
    fi
    echo "${HOST_SUMS_LINE}" >> "${MERGED_SUMS}"
    sort -k 2 "${MERGED_SUMS}" -o "${MERGED_SUMS}"
    gh release upload "${VERSION}" --repo "${REPO}" --clobber \
        "${DIST}/${TARBALL_NAME}" \
        "${MERGED_SUMS}#${SUMS_NAME}"
else
    echo "[release.cut] creating release ${VERSION}"
    echo "${HOST_SUMS_LINE}" > "${MERGED_SUMS}"
    gh release create "${VERSION}" --repo "${REPO}" \
        --title "cook ${VERSION}" \
        --notes "Phase 1 install layout. Host targets uploaded as cuts arrive from each machine; remaining targets land via CI follow-up (see SHI-176 M1.2b)." \
        "${DIST}/${TARBALL_NAME}" \
        "${MERGED_SUMS}#${SUMS_NAME}"
fi

echo "[release.cut] done: ${TARBALL_NAME} on ${REPO}@${VERSION}"
]],
        version
    )

    cook.exec(pipeline, 0)
end

return M
