; ── Keywords ─────────────────────────────────────────────────────

[
  "recipe"
  "config"
  "use"
  "import"
  "end"
  "ingredients"
  "cook"
  "plate"
  "test"
  "using"
  "timeout"
  "should_fail"
] @keyword

; ── Recipe headers ──────────────────────────────────────────────

(explicit_recipe_header
  name: (identifier) @function.builtin)

(explicit_recipe_header
  name: (string) @function.builtin)

(implicit_recipe_header
  name: (identifier) @function.builtin)

; ── Dependencies ────────────────────────────────────────────────

(dependency_list
  (identifier) @function)

(dependency_list
  (string) @function)

; ── Declarations ────────────────────────────────────────────────

(use_declaration
  module: (identifier) @module)

(use_declaration
  module: (string) @module)

(import_declaration
  name: (import_name) @module)

(import_declaration
  path: (path) @string.special.path)

(config_block
  name: (identifier) @type)

(config_block
  name: (string) @type)

; ── Variables ───────────────────────────────────────────────────

(variable_declaration
  name: (variable_name) @variable)

(variable_declaration
  value: (string) @string)

; ── Recipe steps ────────────────────────────────────────────────

(cook_step
  outputs: (string) @string.special)

(using_clause
  command: (string) @string.special)

(plate_step
  command: (string) @string.special)

(test_step
  command: (string) @string)

(test_step
  timeout: (number) @number)

(ingredients_step
  (string) @string)

; ── Lua ─────────────────────────────────────────────────────────

(lua_line
  (lua_code) @none)

(lua_block
  (lua_code) @none)

(inline_lua_block
  (lua_code) @none)

">" @keyword.directive
">{" @keyword.directive
"}" @keyword.directive

; ── Shell ───────────────────────────────────────────────────────

(interactive_command
  (shell_content) @none)

"@" @keyword.directive

(shell_command
  (shell_content) @none)

; ── Punctuation ─────────────────────────────────────────────────

":" @punctuation.delimiter

; ── Strings and literals ────────────────────────────────────────

(string) @string

(number) @number

; ── Comments ────────────────────────────────────────────────────

(comment) @comment
