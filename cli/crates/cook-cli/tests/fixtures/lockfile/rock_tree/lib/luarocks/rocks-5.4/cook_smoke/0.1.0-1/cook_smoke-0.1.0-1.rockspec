package = "cook_smoke"
version = "0.1.0-1"
source = {
   url = "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock",
}
description = {
   summary = "Phase 3 acceptance fixture",
   license = "MIT",
}
dependencies = { "lua >= 5.4" }
build = { type = "builtin", modules = { cook_smoke = "cook_smoke.lua" } }
