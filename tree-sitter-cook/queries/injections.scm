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

; Inject Lua into register block bodies (CS-0072)
(register_block
  (lua_code) @injection.content
  (#set! injection.language "lua"))

; Inject Lua into top-level module_call text (CS-0072). The whole
; statement (`cook_cc.bin("game", { sources = { "src/main.c" } })`)
; is a Lua expression-statement, possibly spanning multiple lines.
(top_level_module_call
  (module_call_text) @injection.content
  (#set! injection.language "lua"))

; Inject bash into shell commands. `$<IDENT>` placeholders interleave
; with shell_content chunks; the `[shell_content placeholder]+` choice
; matches the full sequence so every chunk is captured in one match,
; while `injection.combined` joins the chunks into a single bash
; injection (placeholder bytes are excluded). Without this, a
; shell-quoted string broken by a placeholder — `echo "libfoo: $<all>"`
; → ["echo \"libfoo: ", "\""] — would reach bash as two unterminated
; fragments and lose its @string highlight; combined, bash sees
; `echo "libfoo: " ` and the string is recognized as a whole.

(shell_command
  [(shell_content) @injection.content (placeholder)]+
  (#set! injection.language "bash")
  (#set! injection.combined))

(interactive_command
  [(shell_content) @injection.content (placeholder)]+
  (#set! injection.language "bash")
  (#set! injection.combined))

(shell_block
  [(shell_content) @injection.content (placeholder)]+
  (#set! injection.language "bash")
  (#set! injection.combined))
