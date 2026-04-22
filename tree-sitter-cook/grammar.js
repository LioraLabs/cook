/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: "cook",

  extras: ($) => [/[ \t\r]/],

  externals: ($) => [$._lua_block_content, $._shell_content, $._config_block_content],

  word: ($) => $._bare_identifier,

  rules: {
    source_file: ($) => repeat($._toplevel_item),

    _toplevel_item: ($) =>
      choice(
        $.recipe,
        $.config_block,
        $.use_declaration,
        $.import_declaration,
        $.variable_declaration,
        $.comment,
        $._newline,
      ),

    comment: ($) => token(seq("#", /[^\n]*/)),
    _newline: ($) => /\n/,

    // ── Top-level declarations ─────────────────────────────────

    variable_declaration: ($) =>
      seq(
        field("name", alias($._bare_identifier, $.variable_name)),
        field("value", $.string),
        $._newline,
      ),

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
        "end",
        $._newline,
      ),

    // ── Recipes ────────────────────────────────────────────────

    recipe: ($) =>
      prec.right(
        seq(
          $._recipe_header,
          $._newline,
          repeat($._recipe_item),
          "end",
          optional($._newline),
        ),
      ),

    _recipe_header: ($) =>
      choice($.explicit_recipe_header, $.implicit_recipe_header),

    explicit_recipe_header: ($) =>
      prec(
        1,
        seq(
          "recipe",
          field("name", $._name),
          optional(seq(":", $.dependency_list)),
        ),
      ),

    implicit_recipe_header: ($) =>
      seq(
        field("name", alias($._bare_identifier, $.identifier)),
        ":",
        optional($.dependency_list),
      ),

    dependency_list: ($) => repeat1($._name),

    // ── Recipe body ────────────────────────────────────────────

    _recipe_item: ($) =>
      choice(
        $.ingredients_step,
        $.cook_step,
        $.plate_step,
        $.test_step,
        $.lua_line,
        $.lua_block,
        $.interactive_command,
        $.shell_command,
        $.comment,
        $._newline,
      ),

    ingredients_step: ($) =>
      seq("ingredients", repeat1($.string), $._newline),

    // Cook steps may declare one or more output patterns. A multi-output
    // step (two or more quoted strings before `using`) represents a single
    // invocation that produces all outputs together; it requires a block-form
    // `using` clause.
    cook_step: ($) =>
      seq(
        "cook",
        field("outputs", repeat1($.string)),
        optional($.using_clause),
        $._newline,
      ),

    // TODO(tree-sitter): support the plain-shell block form
    // `using { shell line; shell line }`. This requires a new external
    // scanner (analogous to `_lua_block_content`) that handles brace balancing
    // and nested comments. Tracked as a follow-up. For now only the quoted
    // shell command and `>{ lua }` forms are recognized by the grammar.
    using_clause: ($) =>
      seq(
        "using",
        choice(
          field("command", $.string),
          field("lua", $.inline_lua_block),
        ),
      ),

    inline_lua_block: ($) =>
      seq(">{", alias($._lua_block_content, $.lua_code), "}"),

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
