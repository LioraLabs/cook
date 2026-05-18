package = "cook_cc"
version = "0.6.1-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_cc-0.6.1-1",
}
description = {
   summary  = "Cook C-family (C + C++) native build module",
   detailed = [[
      Blessed Cook module for C and C++ native builds. Provides declarative
      target makers (cc.bin/lib/shared/headers) accepting a `needs` list for
      declarative system-library discovery, low-level primitives
      (cc.compile/archive/link), multi-strategy package discovery
      (cc.find with project / curated / pkg-config / cmake-compat / bare-probe stages),
      project-scoped finder registration (cc.register_finder), a raising
      find convenience (cc.find_or_error), transitive link propagation
      including macOS frameworks, and compile_commands.json generation.

      0.6.1 fixes a probe-key sigil-resolver collision: check probe
      names containing '.' (notably has_header("stdint.h")) were being
      mis-parsed as $<key.field> shape by the engine sigil resolver,
      which splits at the first '.' after the colon section. Names
      are now sanitised to [A-Za-z0-9_+-] (no dot) before embedding
      in probe keys. Applies to all seven cc.checks.* kinds.

      0.6.0 (CS-0076) adds the cc.checks.* feature-test namespace
      (has_header / has_function / has_define / sizeof / endian /
      has_compile_flag / has_link_flag) and cc.config_header for
      CMake-compatible @VAR@ / #cmakedefine / #cmakedefine01
      substitution. Both are layered on the cook.probe substrate
      introduced in 0.5.0 (CS-0075). Check probes use the key shape
      cc:check:<kind>:<name>:<short-fp>; the config_header renderer
      ships vendored alongside the module and is located at register
      time via package.searchpath.

      Specified normatively at §28 of the Cook Standard (v0.6).
   ]],
   homepage   = "https://github.com/lioralabs/cook-modules",
   license    = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = {
   "lua >= 5.4",
   "lua-cjson ~> 2.1",
   "lpeg ~> 1.0",
}
build = {
   type    = "builtin",
   modules = {
     ["cook_cc"]                              = "cook_cc/init.lua",
     ["cook_cc.toolchain"]                    = "cook_cc/toolchain.lua",
     ["cook_cc.cc"]                           = "cook_cc/cc.lua",
     ["cook_cc.targets"]                      = "cook_cc/targets.lua",
     ["cook_cc.finder"]                       = "cook_cc/finder.lua",
     ["cook_cc.compile_db"]                   = "cook_cc/compile_db.lua",
     ["cook_cc.transitive"]                   = "cook_cc/transitive.lua",
     ["cook_cc.version"]                      = "cook_cc/version.lua",
     ["cook_cc._probe_helpers"]               = "cook_cc/_probe_helpers.lua",
     ["cook_cc._check_helpers"]               = "cook_cc/_check_helpers.lua",
     ["cook_cc.checks"]                       = "cook_cc/checks.lua",
     ["cook_cc.config_header"]                = "cook_cc/config_header.lua",
     ["cook_cc.config_header_renderer"]       = "cook_cc/config_header_renderer.lua",
     ["cook_cc.finders"]                      = "cook_cc/finders/init.lua",
     ["cook_cc.finders.pkg_config"]           = "cook_cc/finders/pkg_config.lua",
     ["cook_cc.finders.bare_probe"]           = "cook_cc/finders/bare_probe.lua",
     ["cook_cc.finders.cmake_compat"]         = "cook_cc/finders/cmake_compat.lua",
     ["cook_cc.finders.cmake_compat.hints"]   = "cook_cc/finders/cmake_compat/hints.lua",
     ["cook_cc.finders.header_probe"]         = "cook_cc/finders/header_probe.lua",
     ["cook_cc.finders.tool_config"]          = "cook_cc/finders/tool_config.lua",
     ["cook_cc.finders.raylib"]               = "cook_cc/finders/raylib.lua",
     ["cook_cc.finders.sdl2"]                 = "cook_cc/finders/sdl2.lua",
     ["cook_cc.finders.openal"]               = "cook_cc/finders/openal.lua",
     ["cook_cc.finders.gl"]                   = "cook_cc/finders/gl.lua",
     ["cook_cc.finders.threads"]              = "cook_cc/finders/threads.lua",
     ["cook_cc.finders.zlib"]                 = "cook_cc/finders/zlib.lua",
     ["cook_cc.finders.libcurl"]              = "cook_cc/finders/libcurl.lua",
   },
}
