package = "cook_cc"
version = "0.5.0-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_cc-0.5.0-1",
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

      0.5.0 (CS-0075) migrates internal caching from register-phase
      cook.cache.set/get to first-class cook.probe units. cc:compiler,
      cc:linker-search-dirs, cc:cmake-driver, and cc:find:<n> are now
      proper probes with declared inputs and demand-driven scheduling.
      Target makers expose a new `needs = {...}` field for system-library
      declarations; the older pattern of capturing find_or_error's record
      into a config block and threading fields into the bin call still
      works (find_or_error now returns a sigil-record of probe-value
      placeholders). find_or_error failure becomes demand-driven —
      missing libraries surface at build time rather than register time.

      Specified normatively at §28 of the Cook Standard (v0.5).
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
     ["cook_cc"]                       = "cook_cc/init.lua",
     ["cook_cc.toolchain"]             = "cook_cc/toolchain.lua",
     ["cook_cc.cc"]                    = "cook_cc/cc.lua",
     ["cook_cc.targets"]               = "cook_cc/targets.lua",
     ["cook_cc.finder"]                = "cook_cc/finder.lua",
     ["cook_cc.compile_db"]            = "cook_cc/compile_db.lua",
     ["cook_cc.transitive"]            = "cook_cc/transitive.lua",
     ["cook_cc.version"]               = "cook_cc/version.lua",
     ["cook_cc._probe_helpers"]        = "cook_cc/_probe_helpers.lua",
     ["cook_cc.finders"]               = "cook_cc/finders/init.lua",
     ["cook_cc.finders.pkg_config"]    = "cook_cc/finders/pkg_config.lua",
     ["cook_cc.finders.bare_probe"]    = "cook_cc/finders/bare_probe.lua",
     ["cook_cc.finders.cmake_compat"]         = "cook_cc/finders/cmake_compat.lua",
     ["cook_cc.finders.cmake_compat.hints"]   = "cook_cc/finders/cmake_compat/hints.lua",
     ["cook_cc.finders.header_probe"]  = "cook_cc/finders/header_probe.lua",
     ["cook_cc.finders.tool_config"]   = "cook_cc/finders/tool_config.lua",
     ["cook_cc.finders.raylib"]        = "cook_cc/finders/raylib.lua",
     ["cook_cc.finders.sdl2"]          = "cook_cc/finders/sdl2.lua",
     ["cook_cc.finders.openal"]        = "cook_cc/finders/openal.lua",
     ["cook_cc.finders.gl"]            = "cook_cc/finders/gl.lua",
     ["cook_cc.finders.threads"]       = "cook_cc/finders/threads.lua",
     ["cook_cc.finders.zlib"]          = "cook_cc/finders/zlib.lua",
     ["cook_cc.finders.libcurl"]       = "cook_cc/finders/libcurl.lua",
   },
}
