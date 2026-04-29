/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// tree-sitter-cook claims conformance with Cook Standard v0.4
// (cs-standard/v0.4). See standard/src/content/docs/appendix/A-grammar.mdx
// for the normative grammar this file mirrors.

module.exports = grammar({
  name: "cook",

  extras: ($) => [/[ \t\r]/],

  externals: ($) => [
    $._lua_block_content,
    $._shell_content,
    $._config_block_content,
    $._shell_block_content,
  ],

  word: ($) => $._bare_identifier,

  rules: {
    source_file: ($) => repeat($._toplevel_item),

    _toplevel_item: ($) =>
      choice(
        $.recipe,
        $.chore,
        $.config_block,
        $.use_declaration,
        $.import_declaration,
        $.comment,
        $._newline,
      ),

    comment: ($) => token(seq("#", /[^\n]*/)),
    _newline: ($) => /\n/,

    // ── Top-level declarations ─────────────────────────────────

    use_declaration: ($) => seq("use", field("module", $._name), $._newline),

    import_declaration: ($) =>
      seq(
        "import",
        field("name", alias($._bare_identifier, $.import_name)),
        field("path", $.path),
        $._newline,
      ),

    config_block: ($) =>
      seq(
        "config",
        optional(field("name", $._name)),
        $._newline,
        alias($._config_block_content, $.lua_code),
      ),

    // ── Recipes ────────────────────────────────────────────────

    recipe: ($) =>
      prec.right(
        seq(
          $._recipe_header,
          $._newline,
          repeat($._recipe_item),
        ),
      ),

    _recipe_header: ($) => $.explicit_recipe_header,

    explicit_recipe_header: ($) =>
      prec(
        1,
        seq(
          "recipe",
          field("name", $._name),
          optional(seq(":", $.dependency_list)),
        ),
      ),

    dependency_list: ($) => repeat1($._name),

    // ── Chores ─────────────────────────────────────────────────

    chore: ($) =>
      prec.right(
        seq(
          $.chore_header,
          $._newline,
          repeat($._chore_item),
        ),
      ),

    chore_header: ($) =>
      seq(
        "chore",
        field("name", $._name),
        optional(seq(":", $.dependency_list)),
      ),

    _chore_item: ($) =>
      choice(
        $.inline_lua_line,
        $.inline_lua_block,
        $.lua_line,
        $.lua_block,
        $.module_call,
        $.interactive_command,
        $.shell_command,
        $.comment,
        $._newline,
      ),

    // ── Recipe body ────────────────────────────────────────────

    _recipe_item: ($) =>
      choice(
        $.ingredients_step,
        $.cook_step,
        $.plate_step,
        $.test_step,
        $.inline_lua_line,
        $.inline_lua_block,
        $.lua_line,
        $.lua_block,
        $.module_call,
        $.interactive_command,
        $.shell_command,
        $.comment,
        $._newline,
      ),

    // App. A.4: ingredients_step ::= "ingredients" ingredient+ NEWLINE
    //          ingredient ::= STRING | "!" STRING
    ingredients_step: ($) =>
      seq(
        "ingredients",
        repeat1(choice($.string, $.ingredient_exclude)),
        $._newline,
      ),

    ingredient_exclude: ($) => seq("!", $.string),

    // App. A.4 multi-output rule: when two or more outputs are given,
    // the using_clause MUST be a block (`>{...}` or `{...}`); a bare
    // string is rejected. The single-output form keeps all four shapes
    // (declaration-only, string, lua block, shell block).
    cook_step: ($) =>
      choice(
        seq(
          "cook",
          field("outputs", $.string),
          optional($.using_clause),
          $._newline,
        ),
        seq(
          "cook",
          field("outputs", $.string),
          repeat1(field("outputs", $.string)),
          $.block_using_clause,
          $._newline,
        ),
      ),

    using_clause: ($) =>
      seq(
        "using",
        choice(
          field("command", $.string),
          field("lua", $.using_lua_block),
          field("shell", $.shell_block),
        ),
      ),

    block_using_clause: ($) =>
      seq(
        "using",
        choice(
          field("lua", $.using_lua_block),
          field("shell", $.shell_block),
        ),
      ),

    // App. A.4 `using_lua_block`: the `>{ … }` execute-phase Lua block in a
    // using clause, with §6.4 input/output bindings. Distinct in name from
    // the recipe-body step kind `inline_lua_block` (`>>{ … }`, register-phase).
    using_lua_block: ($) =>
      seq(">{", alias($._lua_block_content, $.lua_code), "}"),

    shell_block: ($) =>
      seq("{", alias($._shell_block_content, $.shell_content), "}"),

    plate_step: ($) => seq("plate", field("command", $.string), $._newline),

    test_step: ($) =>
      seq(
        "test",
        field("command", $.string),
        optional(seq("timeout", field("timeout", $.number))),
        optional("should_fail"),
        $._newline,
      ),

    lua_line: ($) =>
      seq(
        ">",
        alias(token.immediate(/[^{\n][^\n]*/), $.lua_code),
        $._newline,
      ),

    lua_block: ($) =>
      seq(">{", alias($._lua_block_content, $.lua_code), "}", $._newline),

    // §{recipes.lua-steps} register-phase forms: `>>` line and `>>{ ... }`
    // block. Both desugar at the lexer / scanner level — `>>` and `>>{`
    // tokens, then the same brace-balanced LUA_BLOCK_CONTENT collection
    // for the block form.
    inline_lua_line: ($) =>
      seq(
        ">>",
        alias(token.immediate(/[^{\n][^\n]*/), $.lua_code),
        $._newline,
      ),

    inline_lua_block: ($) =>
      seq(">>{", alias($._lua_block_content, $.lua_code), "}", $._newline),

    // App. A.4 module_call: BARE_IDENT "." IDENT_START ...
    // First segment is alphanumeric+underscore only (no hyphens/dots).
    // Second segment must begin with [A-Za-z_]. The remainder of the
    // line is not validated here; Lua-expression-hood is the runtime's
    // concern. Multi-line brace-spanning forms (§ 4.11) are not yet
    // supported by this grammar.
    module_call: ($) =>
      seq(
        alias(
          token(prec(1, /[A-Za-z_][A-Za-z0-9_]*\.[A-Za-z_][^\n]*/)),
          $.module_call_text,
        ),
        $._newline,
      ),

    interactive_command: ($) =>
      seq(
        "@",
        alias(token.immediate(/[^\n]+/), $.shell_content),
        $._newline,
      ),

    shell_command: ($) =>
      seq(alias($._shell_content, $.shell_content), $._newline),

    // ── Primitives ─────────────────────────────────────────────

    _name: ($) => choice(alias($._bare_identifier, $.identifier), $.string),

    _bare_identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_.\-]*/,

    string: ($) => /"[^"]*"/,

    path: ($) => /[^\s\n]+/,

    number: ($) => /[0-9]+/,
  },
});
