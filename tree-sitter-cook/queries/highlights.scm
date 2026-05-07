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
  "as"
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

(test_step
  timeout: (number) @number)

(test_step
  as_name: (string) @string.special)

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

(shell_block
  (shell_content) @none)

; ── Punctuation ─────────────────────────────────────────────────

":" @punctuation.delimiter

; ── Strings and literals ────────────────────────────────────────

(string) @string

(number) @number

; ── Placeholders (§2.11) ────────────────────────────────────────
; A `$<IDENT>` placeholder appears inside string literals and shell
; text. The seq is structured so the brackets and the identifier each
; pick up a distinct highlight — the brackets read as punctuation, the
; identifier as a parameter-style variable. Placement after the broad
; `(string) @string` capture lets these inner-node captures take over.

(placeholder
  "$<" @punctuation.special
  ">" @punctuation.special)

(placeholder
  (placeholder_ident) @variable.parameter)

; ── Comments ────────────────────────────────────────────────────

(comment) @comment
