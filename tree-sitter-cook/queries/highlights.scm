; ── Keywords ─────────────────────────────────────────────────────

[
  "recipe"
  "chore"
  "probe"
  "config"
  "register"
  "use"
  "import"
  "ingredients"
  "cook"
  "plate"
  "test"
  "produce"
  "using"
  "timeout"
  "should_fail"
  "as"
] @keyword

(produce_type) @keyword

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

; ── Probe headers (COOK-67 / §22) ───────────────────────────────

(probe_header
  name: (identifier) @function.builtin)

(probe_header
  name: (string) @function.builtin)

(probe_dep_list
  (identifier) @function)

(probe_dep_list
  (string) @function)

; ── Chore parameters (COOK-36 / §7.1.1) ─────────────────────────

(required_param
  name: (identifier) @variable.parameter)

(defaulted_param
  name: (identifier) @variable.parameter)

(variadic_param
  sigil: _ @operator
  name: (identifier) @variable.parameter)

(defaulted_param
  "=" @operator)

(lua_expr_default
  "(" @punctuation.bracket
  ")" @punctuation.bracket)

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

; ── Top-level module call (CS-0072) ─────────────────────────────

(top_level_module_call
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
