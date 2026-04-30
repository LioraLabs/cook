; ── Keywords ─────────────────────────────────────────────────────

[
  "recipe"
  "chore"
  "config"
  "use"
  "import"
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

; ── Chore headers ───────────────────────────────────────────────

(chore_header
  name: (identifier) @function.builtin)

(chore_header
  name: (string) @function.builtin)

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

(ingredient_exclude
  "!" @operator)

(ingredient_exclude
  (string) @string)

; ── Module call ─────────────────────────────────────────────────

(module_call
  (module_call_text) @function.call)

; ── Lua ─────────────────────────────────────────────────────────

(lua_line
  (lua_code) @none)

(lua_block
  (lua_code) @none)

(using_lua_block
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
