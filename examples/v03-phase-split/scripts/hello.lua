print("[script] hello from a stand-alone Lua build script")
print("[script] cwd reading via cook.sh:", cook.sh("pwd"):gsub("%s+$", ""))
print("[script] cook.env.HOME:", cook.env.HOME)
