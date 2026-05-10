package = "luafilesystem"
version = "1.8.0-1"
source = {
   url = "https://luarocks.org/manifests/hisham/luafilesystem-1.8.0-1.src.rock",
}
description = { summary = "lfs", license = "MIT" }
dependencies = { "lua >= 5.1" }
build = { type = "builtin", modules = { lfs = "lfs.c" } }
