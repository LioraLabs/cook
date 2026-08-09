#!/usr/bin/env bash
# Assert that the cook binary exports the Lua C API, so C rocks can load.
#
# cook embeds Lua statically (mlua "vendored"). A C rock such as lpeg or cjson
# is a shared object that cook dlopens at `require` time, and it resolves
# `lua_*` / `luaL_*` against the host process. A statically linked symbol is
# only visible to dlopen if the executable was linked to export it, which is
# what the -rdynamic in .cargo/config.toml is for.
#
# Nothing about the failure is loud. The binary links, starts, runs a build that
# touches no C rock, and passes every Rust test; it dies only at the first
# `require` of a rock, with `undefined symbol: lua_gettop`. That symptom reads
# like a broken rock rather than a broken cook, and it cost an entire
# milestone's integration testing to a diagnosis of "no C rock can load in
# cook" (COOK-447), which was never true: the binary under test had been built
# from the repo root with --manifest-path, and cargo reads .cargo/config.toml
# from the working directory, so no flag was applied.
#
# The config now lives at the repo root so both spellings pick it up. This gate
# is what keeps that true, since prose in a config file did not.
#
# Usage: check-lua-symbols.sh <path-to-cook-binary>
set -euo pipefail

bin=${1:?usage: check-lua-symbols.sh <path-to-cook-binary>}

if [ ! -x "$bin" ]; then
	echo "check-lua-symbols: no executable at $bin" >&2
	exit 1
fi

if ! command -v nm >/dev/null 2>&1; then
	echo "check-lua-symbols: nm not found; cannot verify exported symbols" >&2
	exit 1
fi

# A rock reaches for both halves of the API, so check one of each rather than
# trusting that a single hit implies the rest.
required=(lua_gettop lua_checkstack luaL_newstate)

case "$(uname -s)" in
Darwin) exported=$(nm -gU "$bin" 2>/dev/null | awk '{print $NF}' | sed 's/^_//') ;;
*) exported=$(nm -D "$bin" 2>/dev/null | awk '$2 == "T" {print $3}') ;;
esac

missing=()
for sym in "${required[@]}"; do
	grep -qx "$sym" <<<"$exported" || missing+=("$sym")
done

if [ ${#missing[@]} -ne 0 ]; then
	cat >&2 <<EOF
check-lua-symbols: $bin exports no Lua C API

  missing: ${missing[*]}

Every C rock (lpeg, cjson, lfs) will fail at require with
'undefined symbol: <one of the above>'. Pure-Lua modules are unaffected, which
is why an ordinary build and test run stays green.

The binary was almost certainly built with a command whose working directory is
not covered by the repo's .cargo/config.toml. Build with 'cook cli.build', or
run cargo from a directory at or below the repo root so the config is found.
EOF
	exit 1
fi

echo "cook exports the Lua C API (${#required[@]} probe symbols present)"
