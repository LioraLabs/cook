; Inject Lua into lua lines and blocks
(lua_line
  (lua_code) @injection.content
  (#set! injection.language "lua"))

(lua_block
  (lua_code) @injection.content
  (#set! injection.language "lua"))

(using_lua_block
  (lua_code) @injection.content
  (#set! injection.language "lua"))

(inline_lua_line
  (lua_code) @injection.content
  (#set! injection.language "lua"))

(inline_lua_block
  (lua_code) @injection.content
  (#set! injection.language "lua"))

; Inject Lua into config block bodies
(config_block
  (lua_code) @injection.content
  (#set! injection.language "lua"))

; Inject bash into shell commands
(shell_command
  (shell_content) @injection.content
  (#set! injection.language "bash"))

(interactive_command
  (shell_content) @injection.content
  (#set! injection.language "bash"))

(shell_block
  (shell_content) @injection.content
  (#set! injection.language "bash"))
